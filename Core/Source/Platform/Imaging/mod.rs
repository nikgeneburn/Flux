use std::path::Path;
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits, GetObjectW, HBITMAP,
    HGDIOBJ,
};
use windows::Win32::UI::Controls::{ILD_TRANSPARENT, IImageList};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHGetImageList, SHIL_EXTRALARGE, SHIL_JUMBO, SHIL_LARGE, SHIL_SMALL, SIIGBF, SIIGBF_BIGGERSIZEOK, SIIGBF_THUMBNAILONLY,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

pub struct Pixels {
    pub Width: u32,
    pub Height: u32,
    pub Data: Vec<u8>,
}

struct BitmapGuard(HBITMAP);

impl Drop for BitmapGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(self.0.0));
            }
        }
    }
}

fn ReadBitmap(Bitmap: HBITMAP) -> Option<Pixels> {
    unsafe {
        let mut Header = BITMAP::default();
        if GetObjectW(HGDIOBJ(Bitmap.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut Header as *mut _ as *mut _)) == 0 {
            return None;
        }
        let Width = Header.bmWidth.unsigned_abs();
        let Height = Header.bmHeight.unsigned_abs();
        if Width == 0 || Height == 0 || Width > 4096 || Height > 4096 {
            return None;
        }
        let mut Info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: Width as i32,
                biHeight: -(Height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut Data = vec![0u8; (Width * Height * 4) as usize];
        let Context = CreateCompatibleDC(None);
        let Lines = GetDIBits(Context, Bitmap, 0, Height, Some(Data.as_mut_ptr() as *mut _), &mut Info, DIB_RGB_COLORS);
        let _ = DeleteDC(Context);
        if Lines == 0 {
            return None;
        }
        Some(Pixels { Width, Height, Data })
    }
}

fn Premultiply(Data: &mut [u8]) {
    let HasAlpha = Data.chunks_exact(4).any(|Pixel| Pixel[3] != 0);
    if !HasAlpha {
        for Pixel in Data.chunks_exact_mut(4) {
            Pixel[3] = 255;
        }
        return;
    }
    let Premultiplied = Data.chunks_exact(4).all(|Pixel| Pixel[0] <= Pixel[3] && Pixel[1] <= Pixel[3] && Pixel[2] <= Pixel[3]);
    if Premultiplied {
        return;
    }
    for Pixel in Data.chunks_exact_mut(4) {
        let Alpha = Pixel[3] as u32;
        for Channel in &mut Pixel[..3] {
            *Channel = ((*Channel as u32 * Alpha + 127) / 255) as u8;
        }
    }
}

pub fn MenuBitmapPixels(Bitmap: HBITMAP) -> Option<Pixels> {
    let mut Image = ReadBitmap(Bitmap)?;
    if Image.Width > 64 || Image.Height > 64 {
        return None;
    }
    Premultiply(&mut Image.Data);
    Some(Image)
}

pub fn IconPixels(Icon: HICON) -> Option<Pixels> {
    unsafe {
        let mut Info = ICONINFO::default();
        GetIconInfo(Icon, &mut Info).ok()?;
        let Color = BitmapGuard(Info.hbmColor);
        let Mask = BitmapGuard(Info.hbmMask);
        if Color.0.is_invalid() {
            return None;
        }
        let mut Image = ReadBitmap(Color.0)?;
        let HasAlpha = Image.Data.chunks_exact(4).any(|Pixel| Pixel[3] != 0);
        if !HasAlpha {
            if let Some(MaskImage) = ReadBitmap(Mask.0) {
                if MaskImage.Width == Image.Width && MaskImage.Height >= Image.Height {
                    for (Pixel, MaskPixel) in Image.Data.chunks_exact_mut(4).zip(MaskImage.Data.chunks_exact(4)) {
                        Pixel[3] = if MaskPixel[0] == 0 { 255 } else { 0 };
                    }
                }
            }
            if Image.Data.chunks_exact(4).all(|Pixel| Pixel[3] == 0) {
                for Pixel in Image.Data.chunks_exact_mut(4) {
                    Pixel[3] = 255;
                }
            }
        }
        Premultiply(&mut Image.Data);
        Some(Image)
    }
}

pub fn ImageListFor(PixelSize: u32) -> u32 {
    match PixelSize {
        0..=16 => SHIL_SMALL,
        17..=32 => SHIL_LARGE,
        33..=48 => SHIL_EXTRALARGE,
        _ => SHIL_JUMBO,
    }
}

pub fn SystemIcon(Index: i32, PixelSize: u32) -> Option<Pixels> {
    if Index < 0 {
        return None;
    }
    unsafe {
        let List: IImageList = SHGetImageList(ImageListFor(PixelSize) as i32).ok()?;
        let Icon = List.GetIcon(Index, ILD_TRANSPARENT.0).ok()?;
        let Result = IconPixels(Icon);
        let _ = DestroyIcon(Icon);
        Result
    }
}

pub fn Thumbnail(Item: &Path, PixelSize: u32, ThumbnailOnly: bool) -> Option<Pixels> {
    let Shell = super::Shell::ShellItem(Item)?;
    unsafe {
        let Factory: IShellItemImageFactory = windows::core::Interface::cast(&Shell).ok()?;
        let Flags = if ThumbnailOnly { SIIGBF(SIIGBF_BIGGERSIZEOK.0 | SIIGBF_THUMBNAILONLY.0) } else { SIIGBF_BIGGERSIZEOK };
        let Bitmap = BitmapGuard(Factory.GetImage(SIZE { cx: PixelSize as i32, cy: PixelSize as i32 }, Flags).ok()?);
        let mut Image = ReadBitmap(Bitmap.0)?;
        Premultiply(&mut Image.Data);
        Some(Image)
    }
}

pub fn ApplicationIcon(PixelSize: u32) -> Option<Pixels> {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW};
    unsafe {
        let Instance = GetModuleHandleW(None).ok()?;
        let Handle = LoadImageW(Some(Instance.into()), windows::core::PCWSTR(1 as *const u16), IMAGE_ICON, PixelSize as i32, PixelSize as i32, LR_DEFAULTCOLOR).ok()?;
        let Icon = HICON(Handle.0);
        let Result = IconPixels(Icon);
        let _ = DestroyIcon(Icon);
        Result
    }
}
