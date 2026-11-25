# somtum-pp

Star rating and pp **as the somtum.fun server pays them**, for tools that run beside the game:
overlays and memory readers ([tosu](https://github.com/somtum-fun/tosu),
[gosumemory](https://github.com/somtum-fun/gosumemory)), replay renderers
([danser-go](https://github.com/somtum-fun/danser-go)), bots.

| play | calculator | same as the server |
|---|---|---|
| vanilla (no RX/AP), any mode | ppy's current pp (rosu-pp `pp-update` + one parity fix, `vendor/rosu-pp`) | official calculator |
| osu! **Relax** | akatsuki-pp-rs `osu_2019` | pp-sidecar |
| **Autopilot**, and Relax on taiko/catch/mania | akatsuki-pp-rs main calculator | pp-sidecar |

The routing and the hit-count handling are the pp-sidecar's (TomYum `sidecars/pp-sidecar`), at the
same akatsuki-pp-rs commit (`591de0d`). **Bump the two together.** `scripts/parity.sh` compares
against a running sidecar. On 2026-11-24 it gave 120/120 equal results (5 maps × 8 Relax/Autopilot
mod combos × 3 hit spreads) against dev.

## Use

**Rust**

```rust
let mut map = somtum_pp::Map::from_bytes(&std::fs::read("map.osu")?)?;
let perf = map.calculate(&somtum_pp::Score { mods: 128, n300: 500, n100: 4, max_combo: 700, ..Default::default() })?;
```

**JavaScript** (`js/`, `@somtum/pp`; WebAssembly embedded, no assets to ship)

```js
import { SomtumMap, RELAX } from '@somtum/pp';
const map = SomtumMap.parse(fs.readFileSync('map.osu'));
map.calculate({ mods: RELAX, accuracy: 98.5, nMiss: 1 }); // { pp, stars, calculatorName: 'relax', ... }
```

**Go** (`go/somtumpp`; WebAssembly on wazero, so it's pure Go and needs no cgo)

```go
m, _ := somtumpp.ParseMap(osuBytes)
s := somtumpp.NewScore(0, somtumpp.Relax); s.Accuracy = 99
p, _ := m.Calculate(s) // p.PP, p.Stars, p.Calculator == somtumpp.Relax2019
```

**C / cgo / C#**: `cargo build --release` gives `libsomtum_pp.{a,so}` / `somtum_pp.dll`; ABI in
`include/somtum_pp.h`. Every struct is `f64`s only.

### Score fields

osu!stable's terms: `n300/n100/n50/nGeki/nKatu/nMiss` as stable counts them per mode, `maxCombo`,
`passedObjects` for a play in progress (omit for the whole map), `accuracy` (0-100) to generate hits
instead ("if FC", "at 98%"), `legacyTotalScore` (ppy uses it for osu! miss estimation), `lazer` +
`clockRate` + `largeTickHits` / `sliderEndHits` for TumThai plays.

## Build

```bash
cargo test --release
scripts/build-wasm.sh        # rebuilds js/wasm.mjs and go/somtumpp/somtum_pp.wasm (committed)
scripts/parity.sh tests/maps/*.osu                 # needs a pp-sidecar on :9000
(cd go && go test ./...)
```

Licence: MIT. rosu-pp and akatsuki-pp-rs are MIT.
