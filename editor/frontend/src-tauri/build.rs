fn main() {
    ensure_windows_icon().expect("generate default icon.ico for Windows builds");
    tauri_build::build()
}

fn ensure_windows_icon() -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::fs;
        use std::path::Path;

        let icon_path = Path::new("icons").join("icon.ico");
        if icon_path.exists() {
            return Ok(());
        }

        fs::create_dir_all("icons")?;
        fs::write(icon_path, default_icon_ico())?;
    }

    Ok(())
}

fn default_icon_ico() -> Vec<u8> {
    // Minimal 1x1 PNG (RGBA) embedded into an ICO container.
    // This is only used as a fallback to keep `cargo test --workspace` happy on Windows.
    const ICON_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
        0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00,
        0x00, 0xb5, 0x1c, 0x0c, 0x02, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78,
        0xda, 0x63, 0xfc, 0xff, 0x1f, 0x00, 0x03, 0x03, 0x02, 0x00, 0xee, 0x4d, 0xe1, 0x64,
        0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    let png_len_u32 = u32::try_from(ICON_PNG.len()).expect("png length fits in u32");
    let mut out = Vec::with_capacity(6 + 16 + ICON_PNG.len());

    // ICONDIR header
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type (1 = icon)
    out.extend_from_slice(&1u16.to_le_bytes()); // count

    // ICONDIRENTRY
    out.push(1); // width
    out.push(1); // height
    out.push(0); // color count
    out.push(0); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bit count
    out.extend_from_slice(&png_len_u32.to_le_bytes()); // bytes in resource
    out.extend_from_slice(&22u32.to_le_bytes()); // image offset (6 + 16)

    // PNG image payload
    out.extend_from_slice(ICON_PNG);

    out
}

