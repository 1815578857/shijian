#[path = "src/icon.rs"]
mod icon;

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    let mut attributes = tauri_build::Attributes::new();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // tauri-build requires a Windows resource icon even with bundle.icon = [].
        // Generate a small uncompressed ICO in OUT_DIR instead of checking in image assets.
        let path = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("缺少构建输出目录"))
            .join("shijian.ico");
        std::fs::write(&path, windows_icon()).expect("无法生成 Windows 应用图标");
        attributes = attributes
            .windows_attributes(tauri_build::WindowsAttributes::new().window_icon_path(path));
    }
    tauri_build::try_build(attributes).expect("无法生成拾笺 Tauri 构建资源");
}

fn windows_icon() -> Vec<u8> {
    let (rgba, size) = icon::rgba();
    let mask_stride = size.div_ceil(32) * 4;
    let image_bytes = 40 + size * size * 4 + mask_stride * size;
    let mut ico = Vec::new();
    for value in [0_u16, 1, 1] {
        ico.extend_from_slice(&value.to_le_bytes());
    }
    ico.extend_from_slice(&[size as u8, size as u8, 0, 0]);
    ico.extend_from_slice(&1_u16.to_le_bytes());
    ico.extend_from_slice(&32_u16.to_le_bytes());
    ico.extend_from_slice(&image_bytes.to_le_bytes());
    ico.extend_from_slice(&22_u32.to_le_bytes());
    ico.extend_from_slice(&40_u32.to_le_bytes());
    ico.extend_from_slice(&(size as i32).to_le_bytes());
    ico.extend_from_slice(&((size * 2) as i32).to_le_bytes());
    ico.extend_from_slice(&1_u16.to_le_bytes());
    ico.extend_from_slice(&32_u16.to_le_bytes());
    ico.extend_from_slice(&0_u32.to_le_bytes());
    ico.extend_from_slice(&(size * size * 4).to_le_bytes());
    ico.extend_from_slice(&[0_u8; 16]);
    for y in (0..size).rev() {
        for x in 0..size {
            let offset = ((y * size + x) * 4) as usize;
            let p = &rgba[offset..offset + 4];
            ico.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    for y in (0..size).rev() {
        let mut mask = vec![0_u8; mask_stride as usize];
        for x in 0..size {
            if rgba[((y * size + x) * 4 + 3) as usize] == 0 {
                mask[(x / 8) as usize] |= 0x80 >> (x % 8);
            }
        }
        ico.extend_from_slice(&mask);
    }
    ico
}
