//! # somtum-pp
//!
//! Star rating and pp as the somtum.fun server pays them, for tools that run beside the game
//! (overlays, memory readers, replay renderers). One routing for everyone:
//!
//! | play                                   | calculator                                   |
//! |----------------------------------------|----------------------------------------------|
//! | vanilla (no RX/AP), any ruleset        | ppy's current pp (rosu-pp `pp-update`)        |
//! | osu! **Relax**                         | akatsuki-pp-rs `osu_2019`                    |
//! | **Autopilot**, Relax on taiko/catch/mania | akatsuki-pp-rs main calculator           |
//!
//! That is the server's own routing (TomYum `TOMYUM_PERFORMANCE__ROUTES`: vanilla → official, relax
//! and autopilot → pp-sidecar) and the sidecar's own split inside it (sidecars/pp-sidecar/src/main.rs
//! `compute`), so an overlay shows the pp the score will be paid. The akatsuki-pp-rs commit is the
//! sidecar's; keep them in step.
//!
//! The Rust API is [`Map`] + [`Score`] → [`Perf`]. The same thing is exported as a C ABI (`spp_*`,
//! see `include/somtum_pp.h`) that builds as a native static/dynamic library (Go via cgo, C, C#) and
//! as WebAssembly (`wasm32-unknown-unknown`, for Node/browsers; see `js/`). Every struct that crosses
//! the ABI is plain `f64`s, so the WebAssembly side reads and writes them as a `Float64Array`.

use std::panic::{catch_unwind, AssertUnwindSafe};

use akatsuki_pp::model::mode::GameMode as AkaMode;
use akatsuki_pp::osu_2019::{stars::OsuDifficultyAttributes as RxDiff, OsuPP as RxPP};
use akatsuki_pp::{
    any::{DifficultyAttributes as AkaDiff, PerformanceAttributes as AkaPerf}, Beatmap as AkaBeatmap, Difficulty as AkaDifficulty,
    Performance as AkaPerformance,
};
use rosu_mods::{GameMod, GameModsIntermode, GameMode as ModsMode};
use rosu_pp::model::mode::GameMode as RosuMode;
use rosu_pp::{
    any::{DifficultyAttributes as RosuDiff, PerformanceAttributes as RosuPerf, Strains},
    Beatmap as RosuBeatmap, Difficulty as RosuDifficulty, Performance as RosuPerformance,
};

pub mod ffi;

/// osu! legacy mod bits used by the routing.
pub mod mods {
    pub const NO_FAIL: u32 = 1 << 0;
    pub const EASY: u32 = 1 << 1;
    pub const HIDDEN: u32 = 1 << 3;
    pub const HARD_ROCK: u32 = 1 << 4;
    pub const DOUBLE_TIME: u32 = 1 << 6;
    pub const RELAX: u32 = 1 << 7;
    pub const HALF_TIME: u32 = 1 << 8;
    pub const NIGHTCORE: u32 = 1 << 9;
    pub const FLASHLIGHT: u32 = 1 << 10;
    pub const SPUN_OUT: u32 = 1 << 12;
    pub const AUTOPILOT: u32 = 1 << 13;
    pub const SCORE_V2: u32 = 1 << 29;
}

/// Which calculator a play goes to. The numbers are part of the ABI (`Perf::calculator`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Calculator {
    /// ppy's pp (vanilla plays).
    Official = 0,
    /// akatsuki-pp-rs `osu_2019` (osu! Relax).
    Relax2019 = 1,
    /// akatsuki-pp-rs main calculator (Autopilot; Relax on taiko/catch/mania).
    Akatsuki = 2,
}

impl Calculator {
    /// The server's routing for a play: mode is the vanilla ruleset (0-3), mods the legacy bits.
    pub fn route(mode: u8, mods: u32) -> Self {
        if mods & mods::RELAX != 0 && mode == 0 {
            Calculator::Relax2019
        } else if mods & (mods::RELAX | mods::AUTOPILOT) != 0 {
            Calculator::Akatsuki
        } else {
            Calculator::Official
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Calculator::Official => "official",
            Calculator::Relax2019 => "relax (akatsuki osu_2019)",
            Calculator::Akatsuki => "akatsuki",
        }
    }
}

/// One play (or a play in progress) in osu!stable's own terms. Hit counts are the score's Count
/// fields as stable keeps them per mode (catch: 300 = fruits, 100 = droplets, 50 = tiny droplets,
/// katu = missed tiny droplets; mania: geki = MAX, katu = 200).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Score {
    /// Vanilla ruleset 0-3. A map of another mode is converted (osu! maps only).
    pub mode: u8,
    /// Legacy mod bits (Relax 128, Autopilot 8192 included).
    pub mods: u32,
    /// Made on lazer (TumThai). Stable plays: false.
    pub lazer: bool,
    /// A lazer rate mod's own speed (DT at 1.2x); `None` = the mod's usual speed.
    pub clock_rate: Option<f64>,
    pub max_combo: u32,
    pub n300: u32,
    pub n100: u32,
    pub n50: u32,
    pub n_geki: u32,
    pub n_katu: u32,
    pub n_miss: u32,
    /// lazer only.
    pub large_tick_hits: Option<u32>,
    /// lazer only.
    pub slider_end_hits: Option<u32>,
    /// Objects judged so far; `None` = the whole map (a finished play, or "if FC").
    pub passed_objects: Option<u32>,
    /// osu!stable's total score (ScoreV1); `None` when unknown. ppy uses it for osu! misses.
    pub legacy_total_score: Option<u32>,
    /// 0..=100. When set, hit counts are generated from it (with `n_miss` misses) instead of read
    /// from the n* fields: "pp at 98%", "if FC".
    pub accuracy: Option<f64>,
}

impl Default for Score {
    fn default() -> Self {
        Self {
            mode: 0,
            mods: 0,
            lazer: false,
            clock_rate: None,
            max_combo: u32::MAX,
            n300: 0,
            n100: 0,
            n50: 0,
            n_geki: 0,
            n_katu: 0,
            n_miss: 0,
            large_tick_hits: None,
            slider_end_hits: None,
            passed_objects: None,
            legacy_total_score: None,
            accuracy: None,
        }
    }
}

/// pp and stars for a score, plus what the overlays show beside them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Perf {
    pub pp: f64,
    pub pp_aim: f64,
    pub pp_speed: f64,
    pub pp_acc: f64,
    pub pp_flashlight: f64,
    pub stars: f64,
    pub max_combo: u32,
    pub effective_miss_count: f64,
    pub calculator: Option<Calculator>,
}

/// Map stats with a play's mods applied (and speed: AR/OD shown as felt).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MapStats {
    pub ar: f64,
    pub od: f64,
    pub cs: f64,
    pub hp: f64,
    pub clock_rate: f64,
    pub n_objects: u32,
    pub mode: u8,
}

/// Strain peaks for the difficulty graph: one combined series per mode.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StrainGraph {
    pub section_len_ms: f64,
    pub values: Vec<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct DiffKey {
    calc: Calculator,
    mode: u8,
    mods: u32,
    rate_bits: u64,
    lazer: bool,
    passed: u32,
}

#[derive(Clone)]
enum CachedDiff {
    Official(RosuDiff),
    Relax(RxDiff),
    Akatsuki(AkaDiff),
}

/// Difficulty is what costs: a live counter asks for pp after every hit with the same mods and a
/// growing `passed_objects`, and "if FC" / "at 99%" ask again for the whole map. Keep a few.
const DIFF_CACHE: usize = 6;

/// A beatmap, parsed once per play.
pub struct Map {
    bytes: Vec<u8>,
    rosu: RosuBeatmap,
    /// Parsed on the first Relax/Autopilot calculation (most plays never need it); per mode, since
    /// akatsuki converts in place.
    aka: Vec<(u8, Option<AkaBeatmap>)>,
    cache: Vec<(DiffKey, CachedDiff)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Parse,
    /// Aspire-style maps that would stall the caller (rosu-pp's suspicion check).
    TooSuspicious,
    /// Can't convert this map to that mode (only osu! maps convert).
    Convert,
    BadMode,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Error::Parse => "beatmap could not be parsed",
            Error::TooSuspicious => "beatmap too suspicious to calculate",
            Error::Convert => "beatmap can't be converted to that mode",
            Error::BadMode => "mode must be 0-3",
        })
    }
}

impl std::error::Error for Error {}

fn rosu_mode(mode: u8) -> Result<RosuMode, Error> {
    Ok(match mode {
        0 => RosuMode::Osu,
        1 => RosuMode::Taiko,
        2 => RosuMode::Catch,
        3 => RosuMode::Mania,
        _ => return Err(Error::BadMode),
    })
}

fn aka_mode(mode: u8) -> AkaMode {
    match mode {
        1 => AkaMode::Taiko,
        2 => AkaMode::Catch,
        3 => AkaMode::Mania,
        _ => AkaMode::Osu,
    }
}

fn finite(v: f64) -> f64 {
    if v.is_finite() && v >= 0.0 {
        v
    } else {
        0.0
    }
}

/// The speed a play's rate mod ran at, when it isn't the mod's usual one (DT/NC 1.5x, HT/DC 0.75x),
/// kept within what lazer lets a player set. Same as the sidecar's `custom_rate`.
pub fn custom_rate(mods: u32, clock_rate: Option<f64>) -> Option<f64> {
    let rate = clock_rate.filter(|r| r.is_finite())?;
    let (rate, usual) = if mods & (mods::DOUBLE_TIME | mods::NIGHTCORE) != 0 {
        (rate.clamp(1.01, 2.0), 1.5)
    } else if mods & mods::HALF_TIME != 0 {
        (rate.clamp(0.5, 0.99), 0.75)
    } else {
        return None;
    };
    ((rate - usual).abs() > 1e-9).then_some(rate)
}

/// The legacy mods as lazer mods with the rate mod at `rate`: osu_2019 takes its clock rate from the
/// mods. Same as the sidecar's `mods_at_rate`.
fn mods_at_rate(mods: u32, rate: f64) -> akatsuki_pp::GameMods {
    let mut lazer = GameModsIntermode::from_bits(mods).with_mode(ModsMode::Osu);
    for m in lazer.iter_mut() {
        match m {
            GameMod::DoubleTimeOsu(m) => m.speed_change = Some(rate),
            GameMod::NightcoreOsu(m) => m.speed_change = Some(rate),
            GameMod::HalfTimeOsu(m) => m.speed_change = Some(rate),
            GameMod::DaycoreOsu(m) => m.speed_change = Some(rate),
            _ => {}
        }
    }
    lazer.into()
}

impl Map {
    /// Parses a .osu file. Fails on maps too suspicious to calculate.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let rosu = RosuBeatmap::from_bytes(bytes).map_err(|_| Error::Parse)?;
        rosu.check_suspicion().map_err(|_| Error::TooSuspicious)?;
        Ok(Self { bytes: bytes.to_vec(), rosu, aka: Vec::new(), cache: Vec::new() })
    }

    /// The map's own mode (0-3).
    pub fn mode(&self) -> u8 {
        self.rosu.mode as u8
    }

    pub fn n_objects(&self) -> u32 {
        self.rosu.hit_objects.len() as u32
    }

    fn aka_map(&mut self, mode: u8, mods: u32) -> Result<&AkaBeatmap, Error> {
        let i = match self.aka.iter().position(|(m, _)| *m == mode) {
            Some(i) => i,
            None => {
                let parsed = AkaBeatmap::from_bytes(&self.bytes).ok().and_then(|mut map| {
                    let target = aka_mode(mode);
                    if map.mode != target {
                        map.convert_mut(target, &mods.into()).ok()?;
                    }
                    Some(map)
                });
                self.aka.push((mode, parsed));
                self.aka.len() - 1
            }
        };
        self.aka[i].1.as_ref().ok_or(Error::Convert)
    }

    fn rosu_map(&self, mode: u8, mods: u32) -> Result<std::borrow::Cow<'_, RosuBeatmap>, Error> {
        let target = rosu_mode(mode)?;
        if self.rosu.mode == target {
            return Ok(std::borrow::Cow::Borrowed(&self.rosu));
        }
        self.rosu
            .convert_ref(target, &mods.into())
            .map_err(|_| Error::Convert)
    }

    fn cached(&mut self, key: &DiffKey) -> Option<CachedDiff> {
        let i = self.cache.iter().position(|(k, _)| k == key)?;
        let entry = self.cache.remove(i);
        let value = entry.1.clone();
        self.cache.push(entry);
        Some(value)
    }

    fn remember(&mut self, key: DiffKey, value: CachedDiff) {
        if self.cache.len() >= DIFF_CACHE {
            self.cache.remove(0);
        }
        self.cache.push((key, value));
    }

    /// The objects a play has reached, within the map; `None` = all of them.
    fn passed(&self, score: &Score) -> Option<u32> {
        score
            .passed_objects
            .filter(|&p| p < self.n_objects())
            .map(|p| p.max(1))
    }

    fn difficulty(&mut self, calc: Calculator, score: &Score) -> Result<CachedDiff, Error> {
        let rate = custom_rate(score.mods, score.clock_rate);
        let passed = self.passed(score);
        let key = DiffKey {
            calc,
            mode: score.mode,
            mods: score.mods,
            rate_bits: rate.map_or(0, f64::to_bits),
            lazer: score.lazer,
            passed: passed.unwrap_or(u32::MAX),
        };
        if let Some(hit) = self.cached(&key) {
            return Ok(hit);
        }
        let value = match calc {
            Calculator::Official => {
                let map = self.rosu_map(score.mode, score.mods)?;
                let mut diff = RosuDifficulty::new().mods(score.mods).lazer(score.lazer);
                if let Some(rate) = rate {
                    diff = diff.clock_rate(rate);
                }
                if let Some(p) = passed {
                    diff = diff.passed_objects(p);
                }
                CachedDiff::Official(diff.checked_calculate(&map).map_err(|_| Error::TooSuspicious)?)
            }
            Calculator::Relax2019 => {
                let mods = match rate {
                    Some(rate) => mods_at_rate(score.mods, rate),
                    None => score.mods.into(),
                };
                let map = self.aka_map(0, score.mods)?;
                let mut calc = RxPP::from_map(map).mods(mods);
                if let Some(p) = passed {
                    calc = calc.passed_objects(p);
                }
                // Stars only: osu_2019 computes difficulty inside calculate(); n300 = 1 keeps the
                // throwaway pp part trivially in range.
                CachedDiff::Relax(calc.n300(1).calculate().difficulty)
            }
            Calculator::Akatsuki => {
                let mode = score.mode;
                let map = self.aka_map(mode, score.mods)?;
                let mut diff = AkaDifficulty::new().mods(score.mods).lazer(score.lazer);
                if let Some(rate) = rate {
                    diff = diff.clock_rate(rate);
                }
                if let Some(p) = passed {
                    diff = diff.passed_objects(p);
                }
                CachedDiff::Akatsuki(diff.calculate(map))
            }
        };
        self.remember(key, value.clone());
        Ok(value)
    }

    /// Stars and max combo for a play's mods, from the calculator that pays it.
    pub fn stars(&mut self, score: &Score) -> Result<(f64, u32), Error> {
        let calc = Calculator::route(score.mode, score.mods);
        Ok(match self.difficulty(calc, score)? {
            CachedDiff::Official(d) => (finite(d.stars()), d.max_combo()),
            CachedDiff::Relax(d) => (finite(d.stars), d.max_combo as u32),
            CachedDiff::Akatsuki(d) => (finite(d.stars()), d.max_combo()),
        })
    }

    /// pp for one score, as the server pays it.
    pub fn calculate(&mut self, score: &Score) -> Result<Perf, Error> {
        rosu_mode(score.mode)?;
        let calc = Calculator::route(score.mode, score.mods);
        let diff = self.difficulty(calc, score)?;
        let passed = self.passed(score);
        let combo = score.max_combo;

        let mut out = match diff {
            CachedDiff::Official(attrs) => {
                let max_combo = attrs.max_combo();
                let mut perf = RosuPerformance::new(attrs).mods(score.mods).lazer(score.lazer);
                if let Some(rate) = custom_rate(score.mods, score.clock_rate) {
                    perf = perf.clock_rate(rate);
                }
                if let Some(p) = passed {
                    perf = perf.passed_objects(p);
                }
                if combo != u32::MAX {
                    perf = perf.combo(combo.min(max_combo));
                }
                perf = perf.misses(score.n_miss);
                perf = match score.accuracy {
                    Some(acc) => perf.accuracy(acc.clamp(0.0, 100.0)),
                    None => {
                        let mut p = perf
                            .n300(score.n300)
                            .n100(score.n100)
                            .n50(score.n50)
                            .n_geki(score.n_geki)
                            .n_katu(score.n_katu);
                        if score.lazer {
                            if let Some(n) = score.large_tick_hits {
                                p = p.large_tick_hits(n);
                            }
                            if let Some(n) = score.slider_end_hits {
                                p = p.slider_end_hits(n);
                            }
                        }
                        if let Some(total) = score.legacy_total_score.filter(|&t| t > 0) {
                            p = p.legacy_total_score(total);
                        }
                        p
                    }
                };
                let attrs = perf.checked_calculate().map_err(|_| Error::TooSuspicious)?;
                let mut out = Perf {
                    pp: attrs.pp(),
                    stars: attrs.stars(),
                    max_combo: attrs.max_combo(),
                    ..Perf::default()
                };
                match attrs {
                    RosuPerf::Osu(o) => {
                        out.pp_aim = o.pp_aim;
                        out.pp_speed = o.pp_speed;
                        out.pp_acc = o.pp_acc;
                        out.pp_flashlight = o.pp_flashlight;
                        out.effective_miss_count = o.effective_miss_count;
                    }
                    RosuPerf::Taiko(t) => out.pp_acc = t.pp_acc,
                    RosuPerf::Catch(_) => {}
                    RosuPerf::Mania(_) => {}
                }
                out
            }
            CachedDiff::Relax(attrs) => {
                let objects = passed.unwrap_or(attrs.n_circles as u32 + attrs.n_sliders as u32 + attrs.n_spinners as u32);
                let max_combo = attrs.max_combo as u32;
                let mods = match custom_rate(score.mods, score.clock_rate) {
                    Some(rate) => mods_at_rate(score.mods, rate),
                    None => score.mods.into(),
                };
                let mut rx = RxPP::from_attributes(attrs).mods(mods);
                if combo != u32::MAX {
                    rx = rx.combo(combo.min(max_combo));
                }
                // Order matters (patcher FFI 1.5.2): passed_objects and misses before accuracy(),
                // which turns the accuracy into counts over the objects it knows of.
                if let Some(p) = passed {
                    rx = rx.passed_objects(p);
                }
                let n_miss = score.n_miss.min(objects);
                rx = rx.misses(n_miss);
                rx = match score.accuracy {
                    Some(acc) => rx.accuracy(acc.clamp(0.0, 100.0) as f32),
                    None => {
                        // As the sidecar: osu_2019 takes hit counts as given, so keep them within
                        // the objects, taking from the 300s first.
                        let n100 = score.n100.min(objects);
                        let n50 = score.n50.min(objects);
                        let n300 = score.n300.min(objects.saturating_sub(n100 + n50 + n_miss));
                        if n300 + n100 + n50 + n_miss == 0 {
                            rx.accuracy(100.0)
                        } else {
                            rx.n300(n300).n100(n100).n50(n50)
                        }
                    }
                };
                let r = rx.calculate();
                Perf {
                    pp: r.pp,
                    pp_aim: r.pp_aim,
                    pp_speed: r.pp_speed,
                    pp_acc: r.pp_acc,
                    stars: r.difficulty.stars,
                    max_combo,
                    effective_miss_count: r.effective_miss_count,
                    ..Perf::default()
                }
            }
            CachedDiff::Akatsuki(attrs) => {
                let max_combo = attrs.max_combo();
                let mut perf = AkaPerformance::new(attrs).mods(score.mods).lazer(score.lazer);
                if let Some(rate) = custom_rate(score.mods, score.clock_rate) {
                    perf = perf.clock_rate(rate);
                }
                if let Some(p) = passed {
                    perf = perf.passed_objects(p);
                }
                if combo != u32::MAX {
                    perf = perf.combo(combo.min(max_combo));
                }
                perf = perf.misses(score.n_miss);
                perf = match score.accuracy {
                    Some(acc) => perf.accuracy(acc.clamp(0.0, 100.0)),
                    None => {
                        let mut p = perf
                            .n300(score.n300)
                            .n100(score.n100)
                            .n50(score.n50)
                            .n_geki(score.n_geki)
                            .n_katu(score.n_katu);
                        if score.lazer {
                            if let Some(n) = score.large_tick_hits {
                                p = p.large_tick_hits(n);
                            }
                            if let Some(n) = score.slider_end_hits {
                                p = p.n_slider_ends(n);
                            }
                        }
                        p
                    }
                };
                let attrs = perf.calculate();
                let mut out = Perf {
                    pp: attrs.pp(),
                    stars: attrs.stars(),
                    max_combo: attrs.max_combo(),
                    ..Perf::default()
                };
                if let AkaPerf::Osu(o) = attrs {
                    out.pp_aim = o.pp_aim;
                    out.pp_speed = o.pp_speed;
                    out.pp_acc = o.pp_acc;
                    out.pp_flashlight = o.pp_flashlight;
                    out.effective_miss_count = o.effective_miss_count;
                }
                out
            }
        };
        out.pp = finite(out.pp);
        out.pp_aim = finite(out.pp_aim);
        out.pp_speed = finite(out.pp_speed);
        out.pp_acc = finite(out.pp_acc);
        out.pp_flashlight = finite(out.pp_flashlight);
        out.stars = finite(out.stars);
        out.calculator = Some(calc);
        Ok(out)
    }

    /// AR/OD/CS/HP with the play's mods (and rate) applied, as felt.
    pub fn stats(&self, score: &Score) -> Result<MapStats, Error> {
        let map = self.rosu_map(score.mode, score.mods)?;
        let mut b = map.attributes();
        b.mods(score.mods);
        let mut d = RosuDifficulty::new().mods(score.mods).lazer(score.lazer);
        if let Some(rate) = custom_rate(score.mods, score.clock_rate) {
            d = d.clock_rate(rate);
        }
        b.difficulty(&d);
        let attrs = b.build();
        let adjusted = attrs.apply_clock_rate();
        Ok(MapStats {
            ar: adjusted.ar as f64,
            od: adjusted.od as f64,
            cs: attrs.cs() as f64,
            hp: attrs.hp() as f64,
            clock_rate: attrs.clock_rate(),
            n_objects: map.hit_objects.len() as u32,
            mode: score.mode,
        })
    }

    /// The difficulty graph: strain peaks per section, one series combining the mode's skills.
    pub fn strains(&self, score: &Score) -> Result<StrainGraph, Error> {
        let map = self.rosu_map(score.mode, score.mods)?;
        let mut d = RosuDifficulty::new().mods(score.mods).lazer(score.lazer);
        if let Some(rate) = custom_rate(score.mods, score.clock_rate) {
            d = d.clock_rate(rate);
        }
        let strains = d.checked_strains(&map).map_err(|_| Error::TooSuspicious)?;
        let section_len_ms = strains.section_len();
        fn sum(series: &[&Vec<f64>]) -> Vec<f64> {
            let len = series.iter().map(|s| s.len()).max().unwrap_or(0);
            (0..len)
                .map(|i| series.iter().map(|s| s.get(i).copied().unwrap_or(0.0)).sum())
                .collect()
        }
        let values = match &strains {
            // Relax has no speed to speak of; aim alone reads truer on its graph.
            Strains::Osu(s) if score.mods & mods::RELAX != 0 => s.aim.clone(),
            Strains::Osu(s) if score.mods & mods::AUTOPILOT != 0 => s.speed.clone(),
            Strains::Osu(s) => sum(&[&s.aim, &s.speed]),
            Strains::Taiko(s) => sum(&[&s.color, &s.rhythm, &s.stamina]),
            Strains::Catch(s) => s.movement.clone(),
            Strains::Mania(s) => s.strains.clone(),
        };
        Ok(StrainGraph { section_len_ms, values: values.into_iter().map(finite).collect() })
    }
}

/// Runs `f`, turning a panic into `default`: a pp of 0 for one frame beats taking the host down.
pub(crate) fn guarded<T>(default: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(default)
}

/// "somtum-pp <version> (rosu-pp pp-update 072ed25, akatsuki-pp-rs 591de0d)".
pub const VERSION: &str = concat!(
    "somtum-pp ",
    env!("CARGO_PKG_VERSION"),
    " (rosu-pp pp-update 072ed25 + somtum fix, akatsuki-pp-rs 591de0d)\0"
);

#[cfg(test)]
mod tests;
