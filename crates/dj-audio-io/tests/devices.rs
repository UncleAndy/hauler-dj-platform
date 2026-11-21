//! The device list and the name the engine resolves have to agree: a window that offers a name the
//! engine cannot open is worse than a text field.

#[test]
fn the_list_names_what_a_resolution_finds() {
    let Ok(devices) = dj_audio_io::output_device_list() else {
        // No backend at all (a bare container): nothing to check, and failing here would say the code is wrong when
        // the machine has no sound card.
        eprintln!("no audio backend on this machine; the list is skipped");
        return;
    };
    for device in &devices {
        assert!(
            !device.name.is_empty(),
            "a device with no name cannot be written into settings"
        );
        assert!(
            matches!(device.channels, 1 | 2 | 4),
            "{} reports {} channels, which the plan does not know how to open",
            device.name,
            device.channels
        );
        assert!(
            device.sample_rate >= 8_000,
            "{} reports {} Hz",
            device.name,
            device.sample_rate
        );
    }
    // The pairs are the device's own: as many stereo pairs as it has channels, and never more.
    for device in &devices {
        assert!(
            device.native_channels >= device.channels,
            "{} reports {} channels but only {} native ones",
            device.name,
            device.channels,
            device.native_channels
        );
        let pairs = dj_audio_io::device_pairs(Some(&device.name));
        let pairs = match pairs {
            Ok(pairs) => pairs,
            Err(error) => panic!("{} has no pairs: {error:?}", device.name),
        };
        assert_eq!(
            pairs.len(),
            device.native_channels / 2,
            "{} has {} native channels and {} pairs",
            device.name,
            device.native_channels,
            pairs.len()
        );
        if let Some((left, right)) = pairs.first() {
            assert_eq!(
                (*left, *right),
                (0, 1),
                "the first pair of {} is not the main one",
                device.name
            );
        }
    }

    // At most one default, and it stands first.
    let defaults = devices.iter().filter(|device| device.default).count();
    assert!(defaults <= 1, "{defaults} devices claim to be the default");
    if let Some(first) = devices.first() {
        if defaults == 1 {
            assert!(first.default, "the default device is not first in the list");
        }
        // The name the list shows has to be a name `resolve_device` finds — that is the promise the window makes.
        let matched = dj_audio_io::resolve_device(Some(&first.name));
        assert!(
            matched.is_ok(),
            "the listed device {} does not resolve: {matched:?}",
            first.name
        );
    }
}
