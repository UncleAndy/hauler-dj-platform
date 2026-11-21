# Effects: evidence (21.11)

What is done, and what proves it.

| Claim | Proof |
|---|---|
| Seven laws of DSP, each from a named source | cargo test -p dj-fx — 17 tests: filter 4, echo 3, roll 3, flanger 1, phaser 1, bitcrusher 2, noise 2, kinds 4 |
| The roll seam does not click | crates/dj-fx/tests/roll.rs::the_seam_does_not_click; the control run without the crossfade steps 0.254 and fails |
| The effect sits in the strip before the fader | crates/dj-mixer/tests/fx_in_strip.rs — 4 cases |
| The patch reaches the engine and comes back | crates/dj-engine/tests/fx.rs — 3 cases |
| The hand reaches the engine | bash scripts/ui.sh --fx-check: knob 0,50; switch on; chooser echo; all cases passed |
| The unit outlives the restart | bash scripts/fx-check.sh — two runs, fresh engine, echo chosen once and played again |

Measured cost: the deck meter went from 149,1 to 86,1 px; the deck and master meters still end on the same line
(417,5). The owner accepted this price on 21.11 in exchange for the block standing under the EQ.

Not done, and named here so it cannot be mistaken for finished: gater, reverb, simple distortion and EQ as an
effect (the rest of DJP-FX-004); post-fader and send routing (DJP-FX-003); the 43-effect matrix (DJP-FX-005);
9 Mixer FX (DJP-FX-006); a chain of effects (DJP-FX-002).
