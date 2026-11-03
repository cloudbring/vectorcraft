use super::*;

fn variance(samples: impl Iterator<Item = (usize, f64)> + Clone) -> f64 {
    let mass: f64 = samples.clone().map(|(_, value)| value).sum();
    let center = samples.clone().map(|(index, value)| index as f64 * value).sum::<f64>() / mass;
    samples.map(|(index, value)| (index as f64 - center).powi(2) * value).sum::<f64>() / mass
}

#[test]
fn gaussian_impulse_variance() {
    let size = 1025;
    for sigma in [1.0, 4.0, 16.0] {
        let mut plane = vec![0.0; size * size];
        plane[(size / 2) * size + size / 2] = 1.0;
        blur_plane(&mut plane, size, size, sigma);
        let mut pixels: Vec<Px16> = (0..size * size)
            .map(|index| {
                let vertical = if index % size == size / 2 { u16::MAX } else { 0 };
                let horizontal = if index / size == size / 2 { u16::MAX } else { 0 };
                [vertical, horizontal, 0, u16::MAX]
            })
            .collect();
        gaussian(&mut pixels, size, sigma);
        for (axis, actual) in [
            ("plane x", variance((0..size).map(|index| (index, f64::from(plane[(size / 2) * size + index]))))),
            ("plane y", variance((0..size).map(|index| (index, f64::from(plane[index * size + size / 2]))))),
            ("rgba x", variance((0..size).map(|index| (index, f64::from(pixels[(size / 2) * size + index][0]))))),
            ("rgba y", variance((0..size).map(|index| (index, f64::from(pixels[index * size + size / 2][1]))))),
        ] {
            let error = 100.0 * (actual / sigma.powi(2) - 1.0);
            eprintln!("sigma={sigma} {axis}: variance={actual:.9}, error={error:+.6}%");
            assert!(error.abs() < 0.5, "sigma={sigma} {axis}: {error:+.6}%");
        }
    }
}

#[test]
fn gaussian_edges_are_zero_extended_without_renormalization() {
    for sigma in [0.2, 1.0, 4.0, 16.0] {
        for (width, height) in [(1, 1), (7, 1), (1, 7), (7, 5)] {
            let source: Vec<f32> = (0..width * height).map(|index| (index % 5 + 1) as f32).collect();
            let mut actual = source.clone();
            blur_plane(&mut actual, width, height, sigma);
            let pad = 512;
            let padded_width = width + 2 * pad;
            let padded_height = height + 2 * pad;
            let mut padded = vec![0.0; padded_width * padded_height];
            for (index, value) in source.iter().enumerate() {
                padded[(index / width + pad) * padded_width + index % width + pad] = *value;
            }
            blur_plane(&mut padded, padded_width, padded_height, sigma);
            for (index, value) in actual.iter().enumerate() {
                let expected = padded[(index / width + pad) * padded_width + index % width + pad];
                assert!((value - expected).abs() < 1e-5, "sigma={sigma} {width}x{height}: {value} != {expected}");
            }
            let mut constant = vec![1.0; width * height];
            blur_plane(&mut constant, width, height, sigma);
            assert!(constant[0] < 1.0, "the corner must fade, not repeat or renormalize");
        }
    }
}

#[test]
fn gaussian_keeps_premultiplied_channels_and_transparent_black() {
    let (width, height) = (61, 47);
    for sigma in [0.2, 1.0, 4.0, 16.0, 64.0] {
        let mut pixels: Vec<u8> = (0..width * height)
            .flat_map(|index| if (13..40).contains(&(index % width)) && (9..32).contains(&(index / width)) { [128, 64, 32, 128] } else { [0; 4] })
            .collect();
        gaussian_rgba(&mut pixels, width, height, sigma);
        for &[red, green, blue, alpha] in pixels.as_chunks::<4>().0 {
            assert_eq!(red, alpha);
            assert!(green.abs_diff(alpha / 2) <= 1 && blue.abs_diff(alpha / 4) <= 1);
            assert!(green <= alpha && blue <= alpha);
            if alpha == 0 {
                assert_eq!([red, green, blue], [0; 3]);
            }
        }
    }
}

#[test]
fn rgba_adapter_matches_the_wide_premultiplied_path() {
    for (width, height) in [(1, 1), (1, 17), (17, 1), (23, 17)] {
        for sigma in [0.2, 1.0, 2.0, 4.0, 8.0, 16.0, 64.0] {
            let mut pixels: Vec<u8> = (0..width * height)
                .flat_map(|index| {
                    let alpha = (index * 47 % 256) as u8;
                    [alpha / 2, alpha / 3, alpha / 5, alpha]
                })
                .collect();
            let mut wide = to16(pixels.as_chunks::<4>().0);
            gaussian(&mut wide, width, sigma);
            gaussian_rgba(&mut pixels, width, height, sigma);
            let expected: Vec<u8> = wide.iter().flat_map(|pixel| pixel.map(to8)).collect();
            assert_eq!(pixels, expected, "{width}x{height}, sigma={sigma}");
        }
    }
}

#[test]
fn invalid_blur_inputs_and_subthreshold_sigmas_leave_pixels_unchanged() {
    for sigma in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 0.0, 0.19] {
        let mut plane = vec![1.0; 12];
        blur_plane(&mut plane, 4, 3, sigma);
        assert_eq!(plane, vec![1.0; 12]);
        let mut pixels = vec![128; 48];
        gaussian_rgba(&mut pixels, 4, 3, sigma);
        assert_eq!(pixels, vec![128; 48]);
    }
    for (width, height) in [(0, 3), (4, 0), (4, 4), (usize::MAX, 3)] {
        let mut plane = vec![1.0; 12];
        blur_plane(&mut plane, width, height, 4.0);
        assert_eq!(plane, vec![1.0; 12]);
        let mut pixels = vec![128; 48];
        gaussian_rgba(&mut pixels, width, height, 4.0);
        assert_eq!(pixels, vec![128; 48]);
    }
    let mut incomplete = vec![[128; 4]; 11];
    gaussian(&mut incomplete, 4, 4.0);
    assert_eq!(incomplete, vec![[128; 4]; 11]);
    box_blur(&mut incomplete, 4, 2);
    assert_eq!(incomplete, vec![[128; 4]; 11]);
}

#[test]
fn box_prefilter_keeps_its_radius_zero_edges_and_alpha() {
    let mut pixels = vec![[0; 4]; 7 * 5];
    pixels[0] = [60_000, 30_000, 15_000, 60_000];
    box_blur(&mut pixels, 7, 2);
    for (index, pixel) in pixels.iter().enumerate() {
        let expected = if index % 7 <= 2 && index / 7 <= 2 { [2400, 1200, 600, 2400] } else { [0; 4] };
        assert_eq!(*pixel, expected);
    }
}

#[test]
#[ignore]
fn gaussian_measurements() {
    let size = 1025;
    for sigma in [1.0, 4.0, 16.0] {
        let mut plane = vec![0.0; size * size];
        plane[(size / 2) * size + size / 2] = 1.0;
        blur_plane(&mut plane, size, size, sigma);
        let actual = variance((0..size).map(|index| (index, f64::from(plane[(size / 2) * size + index]))));
        eprintln!("sigma={sigma}: variance={actual:.9}, error={:+.6}%", 100.0 * (actual / sigma.powi(2) - 1.0));
        let mut pixels: Vec<Px16> = (0..size * size).map(|index| if index % size == size / 2 { [u16::MAX; 4] } else { [0; 4] }).collect();
        gaussian(&mut pixels, size, sigma);
        let actual = variance((0..size).map(|index| (index, f64::from(pixels[(size / 2) * size + index][3]))));
        eprintln!("sigma={sigma} rgba: variance={actual:.9}, error={:+.6}%", 100.0 * (actual / sigma.powi(2) - 1.0));
    }
    let size = 2048;
    let source: Vec<u8> =
        (0..size * size).flat_map(|index| [(index % size % 256) as u8, (index / size % 256) as u8, (index % 256) as u8, 255]).collect();
    for sigma in [2.0, 16.0, 64.0] {
        let mut elapsed = Vec::new();
        for iteration in 0..8 {
            let mut pixels = source.clone();
            let start = std::time::Instant::now();
            gaussian_rgba(std::hint::black_box(&mut pixels), size, size, sigma);
            let duration = start.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(pixels);
            if iteration > 0 {
                elapsed.push(duration);
            }
        }
        elapsed.sort_by(f64::total_cmp);
        eprintln!("2048x2048 sigma={sigma}: median={:.3} ms, samples={elapsed:?}", elapsed[elapsed.len() / 2]);
    }
}
