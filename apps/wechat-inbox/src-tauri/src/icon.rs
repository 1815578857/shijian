// Shared by the build-time Windows resource and the runtime tray; no image assets or codecs.
pub fn rgba() -> (Vec<u8>, u32) {
    let size = 32_u32;
    let mut rgba = vec![0_u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let pixel = ((y * size + x) * 4) as usize;
            let inside = (3..29).contains(&x) && (3..29).contains(&y);
            let line = (9..23).contains(&x)
                && ((10..13).contains(&y) || (16..19).contains(&y) || (22..25).contains(&y));
            let color = if line {
                [255, 247, 233, 255]
            } else if inside {
                [52, 112, 91, 255]
            } else {
                [0, 0, 0, 0]
            };
            rgba[pixel..pixel + 4].copy_from_slice(&color);
        }
    }
    (rgba, size)
}
