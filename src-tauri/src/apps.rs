//! The apps installed on this PC, as Windows lists them under Start >
//! All apps: desktop programs and Store apps alike. Open keys can pick
//! one, and keep it as "shell:AppsFolder\<app id>", the same form VSD
//! Craft's Open Apps uses. Written for Agora.

use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct App {
    pub name: String,
    /// What an Open key stores: shell:AppsFolder\<app id>.
    pub path: String,
    /// A small icon as a PNG data URI, for the list.
    pub icon: Option<String>,
}

/// Size of the icons in the list.
const LIST_ICON: u32 = 32;

/// Every installed app, sorted by name. Runs on its own thread because
/// the Windows shell wants a single-threaded COM apartment.
pub fn list(icon_uri: fn(image::RgbaImage) -> Option<String>) -> Vec<App> {
    std::thread::spawn(move || {
        let mut apps: Vec<App> = win::list(LIST_ICON)
            .into_iter()
            .map(|(name, path, icon)| App { name, path, icon: icon.and_then(icon_uri) })
            .collect();
        apps.sort_by_key(|a| a.name.to_lowercase());
        apps.dedup_by(|a, b| a.path.eq_ignore_ascii_case(&b.path));
        apps
    })
    .join()
    .unwrap_or_default()
}

/// The icon of an installed app ("shell:AppsFolder\..." or any other
/// shell path), `size` pixels square.
pub fn icon(path: &str, size: u32) -> Option<image::RgbaImage> {
    let path = path.to_string();
    std::thread::spawn(move || win::icon(&path, size)).join().ok().flatten()
}

/// True for the paths [`icon`] handles.
pub fn is_shell_path(path: &str) -> bool {
    path.trim().to_ascii_lowercase().starts_with("shell:")
}

#[cfg(windows)]
mod win {
    use windows::core::{Interface, HSTRING, PWSTR};
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
    };
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
        COINIT_DISABLE_OLE1DDE,
    };
    use windows::Win32::UI::Shell::{
        BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, IShellItemImageFactory,
        SHCreateItemFromParsingName, SHGetKnownFolderItem, KF_FLAG_DEFAULT, SIGDN,
        SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
    };

    /// COM for this thread, undone when it goes out of scope.
    struct Com;
    impl Com {
        fn new() -> Self {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            }
            Com
        }
    }
    impl Drop for Com {
        fn drop(&mut self) {
            unsafe { CoUninitialize() }
        }
    }

    fn name(item: &IShellItem, kind: SIGDN) -> Option<String> {
        unsafe {
            let p: PWSTR = item.GetDisplayName(kind).ok()?;
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as _));
            s.filter(|s| !s.trim().is_empty())
        }
    }

    pub fn list(size: u32) -> Vec<(String, String, Option<image::RgbaImage>)> {
        let _com = Com::new();
        let mut out = Vec::new();
        unsafe {
            let Ok(folder) = SHGetKnownFolderItem::<IShellItem>(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None) else {
                return out;
            };
            let Ok(items) = folder.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems) else {
                return out;
            };
            loop {
                let mut got = [None];
                let mut n = 0u32;
                if items.Next(&mut got, Some(&mut n)).is_err() || n == 0 {
                    break;
                }
                let Some(item) = got[0].take() else { break };
                let (Some(title), Some(id)) =
                    (name(&item, SIGDN_NORMALDISPLAY), name(&item, SIGDN_PARENTRELATIVEPARSING))
                else {
                    continue;
                };
                out.push((title, format!("shell:AppsFolder\\{id}"), image(&item, size)));
                // A Start menu can't hold this many; stop rather than run on.
                if out.len() >= 2000 {
                    break;
                }
            }
        }
        out
    }

    pub fn icon(path: &str, size: u32) -> Option<image::RgbaImage> {
        let _com = Com::new();
        let item: IShellItem =
            unsafe { SHCreateItemFromParsingName(&HSTRING::from(path), None) }.ok()?;
        image(&item, size)
    }

    /// The item's icon as RGBA, `size` pixels square.
    fn image(item: &IShellItem, size: u32) -> Option<image::RgbaImage> {
        unsafe {
            let factory: IShellItemImageFactory = item.cast().ok()?;
            let want = SIZE { cx: size as i32, cy: size as i32 };
            let bmp = factory.GetImage(want, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK).ok()?;
            let img = pixels(bmp);
            let _ = DeleteObject(bmp.into());
            let img = img?;
            Some(if img.width() == size && img.height() == size {
                img
            } else {
                image::imageops::resize(&img, size, size, image::imageops::FilterType::Lanczos3)
            })
        }
    }

    unsafe fn pixels(bmp: HBITMAP) -> Option<image::RgbaImage> {
        let mut info: BITMAP = std::mem::zeroed();
        if GetObjectW(bmp.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut info as *mut _ as *mut _)) == 0 {
            return None;
        }
        let (w, h) = (info.bmWidth, info.bmHeight.abs());
        if w <= 0 || h <= 0 || w > 1024 || h > 1024 {
            return None;
        }
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..std::mem::zeroed()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(None);
        let lines = GetDIBits(dc, bmp, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        if lines == 0 {
            return None;
        }
        // The shell hands back premultiplied BGRA; icons with no alpha at
        // all are made opaque.
        let no_alpha = px.chunks(4).all(|p| p[3] == 0);
        for p in px.chunks_mut(4) {
            p.swap(0, 2);
            if no_alpha {
                p[3] = 255;
            } else if p[3] > 0 && p[3] < 255 {
                let a = p[3] as u32;
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        image::RgbaImage::from_raw(w as u32, h as u32, px)
    }
}

#[cfg(not(windows))]
mod win {
    pub fn list(_: u32) -> Vec<(String, String, Option<image::RgbaImage>)> {
        Vec::new()
    }
    pub fn icon(_: &str, _: u32) -> Option<image::RgbaImage> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_paths() {
        assert!(is_shell_path("shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"));
        assert!(is_shell_path("  Shell:AppsFolder\\x"));
        assert!(!is_shell_path("C:\\Windows\\notepad.exe"));
    }
}
