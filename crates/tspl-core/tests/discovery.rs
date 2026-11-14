//! Discovery and spooler behaviour that does not need a printer attached.

use tspl_core::discover::{self, PrinterKind};

/// The Windows spooler is always present. CUPS is not (CI runners, minimal
/// Linux installs), so on Unix this only checks that a missing `lpstat` is an
/// error value rather than a panic, and that discovery still works without it.
#[test]
fn queue_enumeration_succeeds() {
    let result = discover::list_queues();
    #[cfg(windows)]
    assert!(
        result.is_ok(),
        "queue enumeration failed: {:?}",
        result.err()
    );
    #[cfg(not(windows))]
    if let Err(error) = result {
        assert_ne!(error.to_string(), "", "the error explains itself");
        let discovery = discover::discover();
        assert!(
            discovery
                .warnings
                .iter()
                .any(|w| w.starts_with("print queues")),
            "a missing CUPS shows up as a warning: {:?}",
            discovery.warnings
        );
    }
}

#[test]
fn discovery_reports_targets_in_a_consistent_shape() {
    let discovery = discover::discover();

    for printer in &discovery.printers {
        let spec = printer.target_spec();
        match printer.kind {
            PrinterKind::Queue => assert!(spec.starts_with("raw:")),
            PrinterKind::Usb => assert!(spec.starts_with("usb:")),
            PrinterKind::Serial | PrinterKind::BluetoothSerial => {
                assert!(spec.starts_with("serial:"));
            }
        }
        assert_ne!(printer.name, "");
    }
}

#[test]
fn serial_port_listing_does_not_error_out() {
    // No serial ports is a normal state; an unreadable source is not.
    let result = discover::list_serial_ports();
    assert!(
        result.is_ok(),
        "serial enumeration failed: {:?}",
        result.err()
    );
}

#[cfg(windows)]
#[test]
fn raw_send_to_an_unknown_queue_reports_a_spooler_error() {
    use tspl_core::{parse_target, Error};

    let target = parse_target("raw:tspl-rs-definitely-not-a-queue").expect("target");
    let error = target.send(b"probe").expect_err("expected a failure");

    assert!(
        matches!(error, Error::Spooler { .. }),
        "unexpected error variant: {error:?}"
    );
}
