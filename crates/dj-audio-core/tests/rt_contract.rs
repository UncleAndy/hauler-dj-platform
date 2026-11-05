//! The RT contract as an executable assertion (DJP-NFR-001).
//!
//! The whole test binary runs under an allocator that records allocations made inside an
//! `RtScope`, so "the callback must not allocate" fails here instead of surviving review.

use dj_audio_core::{Engine, render_offline, scheduled};
use dj_domain::{DeckId, DjCommand};
use dj_testkit::rt::{self, CountingAllocator, RtScope};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn play(id: u64, at: u64) -> dj_domain::ScheduledCommand {
    scheduled(
        id,
        at,
        DjCommand::Play {
            deck: DeckId::A,
            enabled: true,
        },
    )
}

#[test]
fn the_instrumentation_sees_a_deliberate_allocation() {
    rt::reset();
    {
        let _scope = RtScope::enter();
        let deliberate = vec![0u8; 4_096];
        std::hint::black_box(&deliberate);
    }
    assert!(
        rt::allocations_in_rt() > 0,
        "the counting allocator must notice an allocation, otherwise the next test is vacuous"
    );
}

#[test]
fn processing_a_block_does_not_allocate() {
    let mut engine = Engine::new(48_000, 440.0);
    // Everything that could allocate happens before the measured section.
    engine.submit(play(1, 0));
    engine.submit(scheduled(
        2,
        128,
        DjCommand::SetChannelGain {
            deck: DeckId::A,
            linear_gain: 0.5,
        },
    ));
    let mut block = vec![0.0f32; 256];
    let mut acks: [dj_domain::Ack; 16] = std::array::from_fn(|_| dj_domain::Ack::accepted(0));
    let _ = engine.drain_acks(&mut acks);

    rt::reset();
    {
        let _scope = RtScope::enter();
        for _ in 0..200 {
            engine.process(&mut block);
        }
    }

    assert_eq!(
        rt::allocations_in_rt(),
        0,
        "the audio callback allocated; this is a defect, not a style issue"
    );
}

#[test]
fn the_offline_renderer_only_allocates_outside_the_callback() {
    let mut engine = Engine::new(48_000, 440.0);
    engine.submit(play(1, 10));

    rt::reset();
    let samples = render_offline(&mut engine, 4_800, 256);
    assert_eq!(samples.len(), 4_800);
    // `render_offline` allocates its output buffer up front, before any `process` call, so
    // the measured section must be clean.
    assert_eq!(rt::allocations_in_rt(), 0);
}
