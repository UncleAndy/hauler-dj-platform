//! The mixer must not allocate on the audio path (DJP-NFR-001, same guard as A01).

use dj_mixer::{ChannelSettings, CrossAssign, Mixer};
use dj_testkit::rt::{self, CountingAllocator, RtScope};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn mixing_a_block_does_not_allocate() {
    let mut mixer = Mixer::new(2);
    mixer.prepare(48_000, 256);
    mixer.set_channel(
        0,
        ChannelSettings {
            trim_db: -3.0,
            fader: 0.8,
            assign: CrossAssign::A,
            cue: true,
        },
    );
    mixer.set_channel(
        1,
        ChannelSettings {
            trim_db: 0.0,
            fader: 1.0,
            assign: CrossAssign::B,
            cue: false,
        },
    );
    let block_a = vec![0.3f32; 256 * 2];
    let block_b = vec![0.2f32; 256 * 2];

    rt::reset();
    {
        let _scope = RtScope::enter();
        for _ in 0..50 {
            mixer.begin_block(256);
            mixer.mix_channel(0, &block_a);
            mixer.mix_channel(1, &block_b);
            mixer.finish_block();
            let _ = mixer.master();
            let _ = mixer.meters();
        }
    }
    assert_eq!(
        rt::allocations_in_rt(),
        0,
        "the mixer allocated on the audio path"
    );
}
