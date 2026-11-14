//! Byte-level checks on the encoder.
//!
//! The reference job length is not arbitrary: 102 bytes per row x 1218 rows of
//! raster, plus 135 bytes of commands, is the 4x6 job a real BY-482BT printed
//! (see `docs/protocol.md`, "Worked example").

use tspl_core::{encode, px_to_mm, Config, Frame};

const HEADER_4X6: &str = "SIZE 101.60 mm,152.40 mm\r\n\
GAP 3.00 mm,0.00 mm\r\n\
DIRECTION 1\r\n\
REFERENCE 0,0\r\n\
DENSITY 8\r\n\
SPEED 4\r\n\
CLS\r\n\
BITMAP 0,0,102,1218,0,";

/// Slices the raster payload out of an encoded job.
fn payload(job: &[u8]) -> &[u8] {
    let marker = b"BITMAP ";
    let start = job
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("BITMAP command")
        + marker.len();

    let mut index = start;
    for _ in 0..5 {
        let comma = job[index..]
            .iter()
            .position(|&byte| byte == b',')
            .expect("BITMAP field separator");
        index += comma + 1;
    }

    let end = job[index..]
        .iter()
        .position(|&byte| byte == b'\r')
        .expect("BITMAP terminator");
    &job[index..index + end]
}

fn config_4x6() -> Config {
    Config {
        width_mm: 101.6,
        height_mm: 152.4,
        ..Config::default()
    }
}

#[test]
fn white_4x6_job_matches_reference_layout() {
    let frame = Frame::new(812, 1218).expect("frame");
    let job = encode(&frame, &config_4x6()).expect("encode");

    assert_eq!(job.len(), 124_371, "reference job length");
    assert_eq!(
        std::str::from_utf8(&job[..HEADER_4X6.len()]).expect("utf8"),
        HEADER_4X6
    );

    let raster = &job[HEADER_4X6.len()..HEADER_4X6.len() + 102 * 1218];
    assert!(
        raster.iter().all(|&byte| byte == 0xFF),
        "white raster is all set bits: a cleared bit prints black"
    );

    let tail = std::str::from_utf8(&job[job.len() - 13..]).expect("utf8");
    assert_eq!(tail, "\r\nPRINT 1,1\r\n");
}

#[test]
fn packing_is_msb_first_with_black_as_zero() {
    struct Case {
        name: &'static str,
        width: u32,
        height: u32,
        black: &'static [(u32, u32)],
        want: &'static [u8],
    }

    let cases = [
        Case {
            name: "all black row",
            width: 8,
            height: 1,
            black: &[
                (0, 0),
                (1, 0),
                (2, 0),
                (3, 0),
                (4, 0),
                (5, 0),
                (6, 0),
                (7, 0),
            ],
            want: &[0x00],
        },
        Case {
            name: "leftmost pixel clears high bit",
            width: 8,
            height: 1,
            black: &[(0, 0)],
            want: &[0x7F],
        },
        Case {
            name: "rightmost pixel clears low bit",
            width: 8,
            height: 1,
            black: &[(7, 0)],
            want: &[0xFE],
        },
        Case {
            name: "white row is all ones",
            width: 8,
            height: 1,
            black: &[],
            want: &[0xFF],
        },
        Case {
            name: "second row is second packet",
            width: 8,
            height: 2,
            black: &[(0, 1)],
            want: &[0xFF, 0x7F],
        },
        Case {
            // Dot 8 black, dots 9-11 white, padding bits 12-15 white.
            name: "partial final byte pads with white",
            width: 12,
            height: 1,
            black: &[(8, 0)],
            want: &[0xFF, 0x7F],
        },
    ];

    for case in cases {
        let mut frame = Frame::new(case.width, case.height).expect("frame");
        for &(x, y) in case.black {
            frame.set(x, y, 0);
        }

        let job = encode(&frame, &Config::default()).expect("encode");
        assert_eq!(
            payload(&job),
            case.want,
            "case {:?} produced {:02x?}",
            case.name,
            payload(&job)
        );
    }
}

#[test]
fn size_is_derived_from_the_frame_when_unset() {
    let frame = Frame::new(203, 406).expect("frame");
    let job = encode(&frame, &Config::default()).expect("encode");
    let text = String::from_utf8_lossy(&job);

    assert!(
        text.starts_with("SIZE 25.40 mm,50.80 mm\r\n"),
        "unexpected size command: {text:.40}"
    );
}

#[test]
fn copies_gap_and_settings_are_honoured() {
    let config = Config {
        width_mm: 100.0,
        height_mm: 150.0,
        gap_mm: 0.0,
        copies: 3,
        ..Config::default()
    };

    let job = encode(&Frame::new(8, 8).expect("frame"), &config).expect("encode");
    let text = String::from_utf8_lossy(&job);

    assert!(text.contains("SIZE 100.00 mm,150.00 mm\r\n"));
    assert!(
        text.contains("GAP 0.00 mm,0.00 mm\r\n"),
        "gap 0 = continuous"
    );
    assert!(text.ends_with("PRINT 3,1\r\n"));
}

#[test]
fn bad_configuration_is_rejected() {
    let frame = Frame::new(8, 8).expect("frame");

    let cases: [(&str, Config); 5] = [
        (
            "density",
            Config {
                density: 16,
                ..Config::default()
            },
        ),
        (
            "speed",
            Config {
                speed: 13,
                ..Config::default()
            },
        ),
        (
            "copies",
            Config {
                copies: 100,
                ..Config::default()
            },
        ),
        (
            "width",
            Config {
                width_mm: 120.0,
                ..Config::default()
            },
        ),
        (
            "dpi",
            Config {
                dpi: 10,
                ..Config::default()
            },
        ),
    ];

    for (field, config) in cases {
        let error = encode(&frame, &config).expect_err("expected an error");
        let message = error.to_string();
        assert!(
            message.contains(field),
            "error for {field:?} did not name it: {message}"
        );
    }
}

#[test]
fn normalisation_fills_defaults_without_forcing_a_gap() {
    let normalized = Config {
        width_mm: 0.0,
        height_mm: 0.0,
        gap_mm: 0.0,
        dpi: 0,
        density: 0,
        speed: 0,
        copies: 0,
        skip_blank: false,
    }
    .normalized();

    assert_eq!(normalized.dpi, tspl_core::DEFAULT_DPI);
    assert_eq!(normalized.density, 8);
    assert_eq!(normalized.speed, 4);
    assert_eq!(normalized.copies, 1);
    assert!(
        (normalized.gap_mm - 0.0).abs() < f64::EPSILON,
        "gap stays zero: continuous media is a valid choice"
    );
}

#[test]
fn millimetre_conversion_round_trips() {
    assert!((px_to_mm(812, 203) - 101.6).abs() < 1e-9);
    assert!((px_to_mm(1218, 203) - 152.4).abs() < 1e-9);
    assert!((px_to_mm(203, 203) - 25.4).abs() < 1e-9);
}

/// Every `BITMAP x,y,w,h,` header in a job, as `(x, y, width_bytes, rows)`.
fn bitmap_headers(job: &[u8]) -> Vec<(u32, u32, u32, u32)> {
    let text = String::from_utf8_lossy(job);
    text.match_indices("BITMAP ")
        .map(|(at, _)| {
            let fields: Vec<u32> = text[at + 7..]
                .split(',')
                .take(4)
                .map(|field| field.parse().expect("numeric BITMAP field"))
                .collect();
            (fields[0], fields[1], fields[2], fields[3])
        })
        .collect()
}

fn compact() -> Config {
    Config {
        skip_blank: true,
        ..config_4x6()
    }
}

#[test]
fn compact_jobs_send_only_inked_bands() {
    // Two separate black blocks far apart vertically.
    let mut frame = Frame::new(812, 1218).expect("frame");
    for y in 100..140 {
        for x in 80..200 {
            frame.set(x, y, 0);
        }
    }
    for y in 900..920 {
        for x in 400..410 {
            frame.set(x, y, 0);
        }
    }

    let job = encode(&frame, &compact()).expect("encode");
    let headers = bitmap_headers(&job);
    assert_eq!(
        headers,
        vec![(80, 100, 15, 40), (400, 900, 2, 20)],
        "byte-aligned bands around each block"
    );
    assert!(job.len() < 2_000, "compact job is {} bytes", job.len());
    assert!(job.ends_with(b"PRINT 1,1\r\n"));
}

#[test]
fn compact_bands_merge_short_gaps_and_keep_polarity() {
    // Two rows of ink separated by 3 blank rows: one band, gap kept white.
    let mut frame = Frame::new(16, 10).expect("frame");
    frame.set(0, 2, 0);
    frame.set(15, 6, 0);
    let job = encode(
        &frame,
        &Config {
            skip_blank: true,
            ..Config::default()
        },
    )
    .expect("encode");

    assert_eq!(bitmap_headers(&job), vec![(0, 2, 2, 5)]);
    let data = payload(&job);
    assert_eq!(data.len(), 2 * 5);
    assert_eq!(&data[..2], &[0x7F, 0xFF], "row 2: dot 0 black");
    assert!(
        data[2..8].iter().all(|&b| b == 0xFF),
        "blank rows stay white"
    );
    assert_eq!(&data[8..], &[0xFF, 0xFE], "row 6: dot 15 black");
}

#[test]
fn compact_blank_label_has_no_bitmap() {
    let frame = Frame::new(812, 1218).expect("frame");
    let job = encode(&frame, &compact()).expect("encode");
    assert_eq!(bitmap_headers(&job).len(), 0, "no BITMAP for a blank label");
    assert!(job.ends_with(b"CLS\r\nPRINT 1,1\r\n"));
}
