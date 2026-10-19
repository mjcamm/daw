# The `stem` stretcher

*Brought over 2026-10-19 from the user's `audioperformerv2` project, where it was built against a test gym (synthetic cases with known correct answers, a scorer and a listening dashboard). The gym, the baseline stretchers, `StemPool` and the benchmarks stayed there; mentions of them below refer to that project. Here it is `crates/daw-stretch`, used by audio clips (`audio-clips.md`), which render on the engine's own worker pool instead of `StemPool`.*

Goal: one general-purpose engine that is transparent on drums, tonal parts and full mixes,
and cheap enough for 16 stereo stems in real time. Code: `crates/daw-stretch/src/stem.rs`.

## Pipeline per frame (synthesis hop `hs`)

1. **Time map** gives the analysis centre `p` for output time `o = k·hs` (below).
2. **One real FFT per channel** of the unwindowed, zero-phase frame (offset τ from the centre
   at index τ mod n). The mid spectrum is the channel sum (linearity, no extra FFT).
3. **Analysis from that one FFT.** A centred Hann window and its derivative are 3-tap filters
   in frequency:
   `X_w(m) = r(m)/2 + (r(m−1)+r(m+1))/4`, `X_dw(m) = i·(π/n)·(r(m−1)−r(m+1))/2`.
   Reassigned frequency `ω(m) = 2πm/n − Im(X_dw/X_w)` (Auger & Flandrin 1995) is exact for
   stationary sinusoids and unambiguous for any hop, unlike frame-difference estimates.
4. **Phase-gradient heap integration** on the mid (PGHI; Průša & Holighaus, "Phase Vocoder
   Done Right", 2017), with the gradient rule below. A bucket queue over ¼ dB magnitude steps
   replaces the binary heap: O(1) per operation.
5. **Stereo:** every channel keeps its analysis phase offset from the mid:
   `Y_c = X_c · e^{i(ψ − φ_mid)}`, one shared unit rotation per bin. Inter-channel phase and
   level differences are preserved exactly; identical channels stay identical.
6. Inverse FFT, Hann synthesis window, overlap-add with constant normalisation.

## The gradient rule (the new part)

Ideal time-stretch by `r` maps the time-frequency plane by `(t, f) → (r·t, f)`. The STFT phase
gradient has two components: instantaneous frequency `∂φ/∂t = ω`, and local group delay
`∂φ/∂f`, which says where in the window the energy sits. Pushing the gradient forward through
the map gives:

- `∂ψ/∂t = ω`: frequencies unchanged (the usual phase vocoder).
- `∂ψ/∂f = r · ∂φ/∂f`: **positions within the window scale by r.**

Standard PGHI time-stretching copies `∂φ/∂f` unscaled. Then the field isn't a gradient for
impulses: an impulse at `t0` should sit at `r·t0` in every overlapping frame, but unscaled
group delay puts it at `t0 + (r−1)·p_k`, which moves from frame to frame. Time and frequency
integration paths then disagree, and the heap order decides the outcome, so hits smear.
With the scaled rule the field is curl-free for both sinusoids and impulses, so integration
is path-independent and every frame places a hit at the same output time. They add coherently.

Discretely, the frequency step uses the wrapped analysis phase difference between adjacent
bins, which is unambiguous for |τ| < n/2: `ψ(m±1) = ψ(m) + r·princarg(φ(m±1) − φ(m))`.

Measured (quick set): scaled 6.36 vs unscaled 6.72; onset error on the real kit 1.45 → 0.00 ms.

## Transient lock and the time map

Noise-like hits (claps, hats) don't have a clean group delay, so they still smeared and
pre-echoed. Frames whose window contains a strong onset therefore use slope 1 (no stretch)
in the time map. Analysis hop then equals synthesis hop and the frames rebuild the input exactly.

The locked segment is the line through `(r·q, q)` for onset `q`. That makes the hit land exactly
where a perfect stretch puts it. Linear ramps on both sides return to the nominal map
`p = o/r`. Deviation from the grid is at most `±(n/2)(1 − 1/r)` (~4.6 ms at r = 1.25) and zero at
the hit. Onsets closer than a window are merged. Ramps never overlap, so the map is continuous.

Onset detection: 128-sample blocks of first-difference (high-passed) energy, rising
`onset_db` (9 dB) over the mean of the previous ~30 ms. A frame only asks about onsets
inside its own window, so this adds no latency beyond the window itself.

## Streaming (`StemStream`, `crates/daw-stretch/src/stream.rs`)

The engine is real-time. The offline `Stem::stretch` is a loop over it, so the gym
measures exactly what ships.

```rust
let mut s = StemStream::new(&Stem::default(), channels, max_block);
s.set_ratio(1.08);         // any time; output/input length (> 1 = slower), 1.0 = bypass
s.seek(input_pos);         // instant: the first block after it is already correct
s.prime(&clip);            // optional: do the seek's start-up work now, not in the first block
s.process(&clip, &mut out);       // each audio block, any size <= max_block
let grid = s.input_position();    // clip position on the musical grid, for playheads/sync

let cue = s.cue(&clip, hot_cue_pos);  // off the audio thread / ahead of time (allocates)
s.launch(&cue);                       // instant, allocation-free: copies the primed state
s.is_bypassed();                      // ratio exactly 1 and hand-over done

// Many stems across cores; the audio thread works too.
let pool = StemPool::new(streams, channels, max_block, workers);
pool.process(&clips, block_len);      // then read pool.slot(i).out, mix
pool.slot(i).stream.set_ratio(r);     // control between blocks
```

- The source is the clip in memory (`&[&[f32]]`, one slice per channel), passed on every call.
  Lookahead into it costs nothing, so there's **no latency**: a seek primes the frames reaching
  back before the position from the clip itself. Reads past either end are silence.
- A tempo change applies from the next frame, audible within about one window (~35 ms).
- `process`, `seek` and `prime` never allocate (tested with a counting allocator). All buffers
  are pre-faulted at construction, so the first frames don't take page faults. FFTs use preallocated scratch, because the libraries' plain
  `process` allocates on every call.
- The output doesn't depend on block slicing (tested). The time map works in input time with
  the current ratio, so the transient lock keeps working while the tempo moves.

**Cues.** `cue(clip, pos)` returns a seeked and primed snapshot. `launch(&cue)` copies it in
without computing frames or allocating, so launching from a hot cue costs about as much as an
ordinary block. If the tempo changed since the cue was built, its start-up frames reflect the
old tempo for the first ~35 ms, like a tempo change during playback. Tested: a cue launch
produces exactly what seek + prime would.

**Bypass at ratio 1.** At exactly 1.0 the input passes through bit-exact for almost no CPU.
Entering, each bin's synthesis phase glides from the stretched phase to the input's over 16
frames (~190 ms). An abrupt switch cancels bins whose phases drifted half a turn: the test
measured a −1.9 dB dip on a tone, which can be far deeper. Then frames stop, and the raw input
fills in exactly as their tails fade. A `cover` ring tracks how much of each output sample the
frames provide, so the fill is exact. Leaving, frames restart from the input's own phases
(which match the raw input) one per hop, with the raw input filling the gap. There's no
start-up burst, even if every stem leaves bypass on the same master-tempo move. Seeking at
ratio 1 starts directly in bypass. `bypass=false` turns it off.

**Pool.** `StemPool` owns the streams and persistent worker threads. Each block the audio
thread publishes the sources, wakes the workers and works itself: all claim streams from one
counter. A worker woken late just does less, so a block is never slower than doing it all on
the audio thread. Workers spin briefly then park between blocks. Each stream's lock is only
taken by the thread that claimed it, so it never contends. Tested: identical output to
processing the streams one after another. Worker threads run at normal priority; giving them
real-time priority is a platform-specific host decision.

**Host guidance for many streams.** Work comes in bursts: one frame (~60 µs stereo) every 512
output samples, and four frames when a stream starts. With 16 streams:
- Streams stagger automatically: each new one takes the next of 16 frame phases, so at
  128-sample blocks about 4 streams compute per block instead of all 16. `set_stagger`
  overrides this (the offline path uses `set_stagger(0, 1)` so gym results are reproducible).
- Launching clips together: call `seek` + `prime` for each one in the blocks before the launch
  beat (quantised launches are known ahead), so the launch block costs nothing extra.

### Real-time benchmark vs Signalsmith

`cargo run --release -p stretch --example realtime` against `tools/bench-signalsmith-rt.exe`
(`scripts/bench-signalsmith-rt.cpp`, built by `scripts/setup-references.sh`). Same 20 s mix,
per-block timing, tempos moving ±8%, high process priority, 2026-10-12. Percentages are the
worst block as a share of its real-time budget.

| 16 stems, launched together | stem (default + primed) | Signalsmith (split + primed) |
|---|---|---|
| 128-sample blocks: mean / p99 / worst | 0.15 / 0.29 / 0.50 ms (17%) | 0.44 / 1.39 / 1.75 ms (60%) |
| 512-sample blocks: mean / p99 / worst | 0.61 / 1.07 / 1.20 ms (10%) | 1.72 / 4.04 / 4.45 ms (38%) |
| Launch cost per stream (seek/prime vs outputSeek) | ~0.24 ms | ~2 ms |

Same benchmark, the newer features (16 stems, 128-sample blocks, worst block / mean):

| | Worst | Mean |
|---|---|---|
| All launched in one block, naive | 4.2–4.3 ms (145%) | 0.16 ms |
| All launched from cues | 0.52–0.60 ms (18–21%) | 0.16 ms |
| All at tempo 1.0 (bypass) | 0.09–0.27 ms | 0.010 ms |
| Pool, 4 threads (primed) | 0.26–0.27 ms (9%) | 0.06 ms |
| Pool, 8 threads | 0.25–0.26 ms; at 512-sample blocks 0.40 ms (3.5%) vs 1.2 single-threaded | 0.06 ms |

At small blocks, 8 threads gain nothing over 4: waking threads costs about what it saves.

Without priming, stem's worst block (all 16 launching at once) is ~4.5 ms; without
`splitComputation`, Signalsmith's p99 at 128-sample blocks is 245% of budget.

## Parameters (`-s "stem:key=value,..."`)

| Key | Default | Meaning |
|---|---|---|
| `frame` | 2048 | FFT size. 2048 beat 1024 (bass pitch error) and 4096 (smeared hits); rechecked with the lock on and vocal/string cases: 3072 and 4096 still score worse. |
| `overlap` | 4 | `frame / hs`. 8 scored the same at half the speed. |
| `scale` | true | Scale group delay by r (false = plain PGHI). |
| `tol` | −70 | dB below frame peak where bins stop being tracked. |
| `lock` | true | Transient lock. |
| `onset` | 9 | Onset threshold, dB. 6 over-triggers on bass; 12 is equivalent. |

## Open problems, roughly by expected gain

- **Bass vs hats resolution.** One frame size is a compromise (`bass` transient error ~4.3).
  Candidates: multi-resolution with phase hand-off at shared frame centres (a stationary
  sinusoid's zero-phase STFT phase at a frame centre doesn't depend on window length), or
  per-frame window switching.
- **Residual pre-echo** of −45..−75 dB in the 12 ms before hits, from locked frames whose phases
  aren't exactly analysis-consistent. Likely masked, but it can be removed.
- **Vibrato** is stretched along with everything else (inherent; see docs/scoring.md).
- **Speed:** see Performance below. Next: streaming API with worst-case-per-block timing.
- **Bass:** Rubber Band R2 beats stem on both bass stems (docs/benchmarks.md).

## Performance

`cargo run --release -p stretch --example bench -- [file.wav]` (default: a 20 s song excerpt).
Compare with `tools/bench-signalsmith.exe [file.wav]` (in-process, same timing method; built by
`scripts/setup-references.sh`). One thread, stereo, 2026-10-12:

| | Full mix | Vocal stem (gaps) |
|---|---|---|
| stem | 195x real time (5.1 ms per second) | 332x |
| Signalsmith default | 47x | 62x |
| Signalsmith `presetCheaper` | 92x | |

16 stereo stems ≈ 8% of one core.

What made it fast (101x → 195x, output unchanged: gym scores within ±0.05):
- `fastmath.rs`: polynomial atan2 (1e-5 rad), sin/cos (2e-6), dB from float bits (0.03 dB, enough
  for ¼ dB buckets), and rounding by the 1.5·2²³ trick, because baseline x86-64 `round()` is a
  libm call.
- Branch-free loops (selects instead of `if`/`match`) so the compiler vectorises them. The
  rotation loop alone gave +17%.
- Bucket queue as linked lists in a preallocated pool, with bucket indices computed per frame.
- Frames quieter than −110 dBFS skip the FFTs entirely, which matters for stems.

- Identical channels (mono material stored as stereo, common for stems) are analysed and
  synthesised once: +10% on such stems, exact (tested against mono processing).

Tried, not worth it:
- AVX2 build: +6%. The remaining cost is the sequential PGHI loop, about 3000 dependent queue
  operations per frame, which is the floor for exact PGHI.
- Skipping queue pushes whose neighbours are done: no gain.
- Overlap 3 instead of 4: 31% faster but quality 5.94 vs 4.82.
- Time-integrating stable bins and running the heap only for rising ones: 21–36% faster, but
  the sung voice's pitch error doubled (11.4 vs 5.8 cents) and chord coherence suffered. It
  would only be worth it as an opt-in eco mode for CPU-starved machines.
