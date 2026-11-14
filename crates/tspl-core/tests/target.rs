//! Frame, dithering and target handling.

// Counting bytes by hand is deliberate: this crate takes no dependencies, so
// the `bytecount` crate clippy suggests is not available.
#![allow(clippy::naive_bytecount)]

use std::path::PathBuf;

use tspl_core::{normalize_port, parse_target, Frame, Target};

#[test]
fn frame_rejects_degenerate_shapes() {
    assert!(Frame::new(0, 10).is_err());
    assert!(Frame::new(10, 0).is_err());
    assert!(Frame::from_gray(4, 4, vec![0; 15]).is_err());
    assert!(Frame::from_gray(4, 4, vec![0; 16]).is_ok());
}

#[test]
fn frame_bounds_are_forgiving() {
    let mut frame = Frame::new(4, 4).expect("frame");
    frame.set(0, 0, 0);
    frame.set(99, 99, 0);

    assert_eq!(frame.get(0, 0), 0);
    assert_eq!(frame.get(99, 99), 255, "out of bounds reads as white");
}

#[test]
fn dither_keeps_black_and_white_and_mixes_grey() {
    let black = Frame::filled(16, 16, 0).expect("frame").dither();
    assert!(black.gray().iter().all(|&value| value == 0));

    let white = Frame::filled(16, 16, 255).expect("frame").dither();
    assert!(white.gray().iter().all(|&value| value == 255));

    let grey = Frame::filled(64, 64, 128).expect("frame").dither();
    let dark = grey.gray().iter().filter(|&&value| value == 0).count();
    assert!(
        (1..grey.gray().len()).contains(&dark),
        "mid grey must produce both tones, got {dark} black pixels"
    );
}

#[test]
fn test_card_is_printable_and_deterministic() {
    let first = Frame::test_card(812, 1218).expect("card");
    let second = Frame::test_card(812, 1218).expect("card");
    assert_eq!(first, second);

    let black = first.gray().iter().filter(|&&value| value == 0).count();
    assert!(black > 10_000, "test card should be mostly marked");
}

#[test]
fn targets_parse_all_three_kinds() {
    assert_eq!(
        parse_target("raw:BY-482BT-RAW").expect("raw"),
        Target::Raw("BY-482BT-RAW".to_string())
    );
    assert_eq!(
        parse_target(r"file:C:\temp\job.tspl").expect("file"),
        Target::File(PathBuf::from(r"C:\temp\job.tspl")),
        "the drive-letter colon must survive"
    );
    assert_eq!(
        parse_target("serial:COM5").expect("serial"),
        Target::Serial("COM5".to_string())
    );
    assert_eq!(
        parse_target("  RAW:Printer 1  ").expect("case and spaces"),
        Target::Raw("Printer 1".to_string())
    );
}

#[test]
fn targets_reject_junk() {
    for spec in ["", "raw", "raw:", "lpt:thing"] {
        assert!(
            parse_target(spec).is_err(),
            "{spec:?} should not parse as a target"
        );
    }
}

#[test]
fn file_target_writes_the_job_and_creates_directories() {
    let dir = std::env::temp_dir().join(format!("tspl-core-test-{}", std::process::id()));
    let path = dir.join("nested").join("job.tspl");

    let job = b"SIZE 4,6\r\nPRINT 1,1\r\n";
    Target::File(path.clone()).send(job).expect("send to file");

    assert_eq!(std::fs::read(&path).expect("read back"), job);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn empty_jobs_are_refused() {
    let target = Target::File(std::env::temp_dir().join("never-written.tspl"));
    assert!(target.send(&[]).is_err());
}

#[test]
fn thumbnail_downscales_without_upscaling() {
    // 200x100 into 20x20 keeps the aspect ratio.
    let frame = Frame::filled(200, 100, 0).expect("frame");
    let thumb = frame.thumbnail(20, 20);
    assert_eq!((thumb.width(), thumb.height()), (20, 10));
    assert!(thumb.gray().iter().all(|&value| value == 0));

    // Never upscales.
    let small = Frame::filled(10, 10, 128).expect("frame");
    assert_eq!(small.thumbnail(100, 100), small);

    // Box averaging keeps edges: left half black, right half white.
    let mut split = Frame::filled(4, 4, 255).expect("frame");
    for y in 0..4 {
        for x in 0..2 {
            split.set(x, y, 0);
        }
    }
    let thumb = split.thumbnail(2, 2);
    assert_eq!(thumb.get(0, 0), 0);
    assert_eq!(thumb.get(1, 0), 255);
}

#[test]
fn windows_com_ports_are_namespaced() {
    let normalized = normalize_port("COM7");
    #[cfg(windows)]
    assert_eq!(normalized, r"\\.\COM7");
    #[cfg(not(windows))]
    assert_eq!(normalized, "COM7");

    #[cfg(windows)]
    assert_eq!(normalize_port(r"\\.\COM12"), r"\\.\COM12");
}
