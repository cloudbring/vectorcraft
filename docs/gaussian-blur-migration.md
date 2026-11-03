# Gaussian blur migration

**Last updated:** 2026-11-03. **Scope:** artcraft-algorithms #2, VectorCraft CPU effects.

## Implementation and compatibility

`crates/effects/src/pixel.rs` delegates both the floating-point `blur_plane` and
premultiplied RGBA Gaussian paths to `craft-blur` at commit
`d98d0605bb63a0862330f3885605ec1def323643`, from branch
`algo/2-craft-blur-midband`. The dependency has a TODO to repoint to a pinned
main revision after that branch lands.

`Quality::Recursive` is deliberate: unlike `RecursiveRaw`, it preserves the
crate's tighter impulse-shape bound in the 2.5–8 band. It uses the crate's
variance-calibrated sampled Exact path below σ=8, including tiny σ, and the
variance-calibrated recurrence above it. VectorCraft has no LightCraft V3
bit-identity requirement that would justify the raw mid-band recurrence.

Planes retain zero extension (`Edge::Zero`); packed RGBA retains transparent
zero extension (`Edge::Transparent`). There is no border renormalization,
unpremultiplication, color-space conversion or change to radius-to-sigma
mapping. The existing no-op threshold below 0.2 px, nonfinite-sigma identity,
65,536 px sigma ceiling, and malformed-size no-ops remain. RGBA8 still enters
the filter at ×257 precision and rounds through u16 before converting back to
u8. Its adapter avoids a redundant u16 image allocation; a bit-equality test
checks it against the wide path at seven sigmas and four image layouts.

This also reaches Gaussian-dependent artistic, brush, edge, pixelate, paint,
sharpen, sketch and texture effects. CPU rasterized Gaussian Blur and Feather
call the RGBA adapter; cached Drop Shadow (including Darkness) and Outer Glow
call the plane adapter. Renderer-native vello filters, including Inner Glow,
are not implementations in `crates/effects` and are unchanged.

Smart Blur's explicit box prefilter now uses `craft_blur::box_blur`, retaining
its radius and transparent extension. The shared box accumulates in f64 and
rounds to u16 after both axes, rather than between axes. The reviewed one-pixel
golden change below comes from that rounding change, not Gaussian calibration.
Radial Blur and Smart Blur's edge-aware sampling algorithms are otherwise
unchanged. No application-owned Gaussian or box implementation remains in
`crates/effects`; the transpose helper remains for morphological dilation.

VectorCraft versions its file envelope and document schema, not the rendering
algorithm. No documented rendering-version/cache-persistence mechanism exists
to bump. Existing effects therefore render with corrected widths on reopening;
there is no legacy-look flag. Runtime raster caches are process-local. File
format versions are unchanged because their layout and parameters did not change.

## Measured variance

Measured before editing the blur implementation, then repeated after migration.
The f32 fixture is a centered impulse in a 1025×1025 zero plane. The u16 RGBA
fixture uses orthogonal impulse lines to measure each axis without collapsing
the 2D impulse into a handful of quantized taps. Variance uses normalized
discrete second moments about the measured centroid. The regression checks
both axes and requires absolute relative error below 0.5%.

| σ (px) | Old f32 variance | Old f32 error | New f32 variance | New f32 error | Old u16 error | New u16 error |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 0.666666667 | −33.333333% | 1.000000019 | +0.000002% | −33.333333% | −0.007630% |
| 4 | 14.721140797 | −7.992870% | 16.000000258 | +0.000002% | −8.321317% | +0.005913% |
| 16 | 250.717976517 | −2.063290% | 255.999999124 | −0.000000% | −2.110653% | −0.355983% |

The old ideal three-box variances were 2/3, 44/3 and 752/3, respectively
(−33.3333%, −8.3333%, −2.0833%). The measured f32 differences include the old
running sum's rounding residue in the long zero tails. New f32 error is below
0.000003%; worst observed u16 error is 0.355983%. These are filter-accuracy
measurements, not a claim that a cropped/quantized RGBA8 point has exact variance.

## Reviewed golden changes

The repository's renderer goldens are authored pixel samples and tolerances in
`crates/render/tests/it/golden.rs`, not PNG snapshots with an update command.
All 22 existing renderer golden tests pass without changing their expectations.
No binary golden assets are added or rewritten.

Exactly one of the three FNV-1a pixel hashes in
`pixel::tests::smart_blur_normal_mode_keeps_its_pixels` changes:

| Smart Blur parameters | Old hash | New hash | Maximum channel delta | Changed pixels |
|---|---:|---:|---:|---:|
| defaults | 11495339692959753241 | unchanged | 0/255 | 0 |
| radius 2, threshold 10, high quality | 14281410954765351056 | unchanged | 0/255 | 0 |
| radius 12, threshold 40, low quality | 5111933201350526354 | 3831106864807608817 | 1/255 | 1 of 1920 |

The old hash was reproduced with an independent integer box reference that
rounded after each axis before accepting the new expected hash. Normal-mode
default/explicit-mode equality remains asserted.

Additionally, six 100×100 fixtures using the existing renderer golden helper
were captured before and after, then compared channel-by-channel as straight
RGBA over white. These diagnostic captures are not new committed goldens:

| Effect | Parameters beyond defaults | Maximum channel delta | Changed pixels |
|---|---|---:|---:|
| Gaussian Blur | radius 8 | 4/255 | 2436 |
| Feather | radius 8 | 12/255 | 1232 |
| Drop Shadow | blur 8 | 3/255 | 1353 |
| Drop Shadow, Darkness | blur 8, shade darkness | 3/255 | 1353 |
| Outer Glow | blur 8 | 0/255 | 0 |
| Inner Glow | blur 8 | 0/255 | 0 |

Default screen-mode glows on white do not expose a visible delta; the zero for
Outer Glow is not a bit-identity claim about the migrated plane filter. The
renderer FX tests separately cover glows, shadows, raster caching and feather.

## Performance and validation

The ignored `gaussian_measurements` test measures `gaussian_rgba` on a 2048²
layer, including allocations and precision conversions. Input cloning is
outside the timed section. Each sigma has one warm-up and seven measured runs;
the reported value is their median. Filtering is single-threaded. Baseline and
post-migration use the same rustup stable release build and fixture. The host
is shared and heavily loaded, so these are diagnostic timings, not a reliable
cross-app speed ranking.

| σ (px) | Before (ms) | After (ms) |
|---:|---:|---:|
| 2 | 75.147 | 79.684 |
| 16 | 72.467 | 59.577 |
| 64 | 73.472 | 59.592 |

The effects suite passes 378 tests (0 failures, 3 ignored); the renderer FX
suite passes 76 (0 failures, 2 ignored), and all 22 renderer goldens pass.
The separately invoked both-axis variance regression and release measurement
test each pass. Scoped all-target clippy with warnings denied and scoped fmt
check pass. The layer gate reports 28 crates and no violations. The migration
removes 125 implementation lines and adds 53 in `pixel.rs` (net −72), excluding
the new regression/measurement module and documentation.

```sh
export PATH="$(dirname "$(rustup which cargo)"):$PATH"
export CARGO_TARGET_DIR="$PWD/target-2"
cargo test -p vectorcraft-effects
cargo test -p vectorcraft-effects gaussian_impulse_variance -- --nocapture
cargo test -p vectorcraft-effects --release gaussian_measurements -- --ignored --nocapture
cargo test -p vectorcraft-render --lib fx::
cargo test -p vectorcraft-render --test it golden::
cargo clippy -p vectorcraft-effects --all-targets -- -D warnings
cargo fmt -p vectorcraft-effects --check
cargo xtask layers
```

Workspace-wide CI, application perf budgets and wasm gates are intentionally
not run for this scoped task. The dedicated `target-2` directory is deleted
after validation, as requested.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-03 | patch | Replace quantized Gaussian boxes and the explicit box prefilter with craft-blur; record measured accuracy, golden deltas and scoped validation. |
