//! The C ABI. Native: `include/somtum_pp.h`. WebAssembly: the same exports, plus `spp_alloc` /
//! `spp_free_bytes` so the host can hand over the .osu bytes.
//!
//! Every struct is `f64`s only (counts too; they're exact up to 2^53), so a WebAssembly host reads
//! and writes them as a `Float64Array`, and Go/C# need no layout care. Field order is the ABI:
//! append only, never reorder.

use crate::{guarded, Map, Score};

/// `SppScore`. Unset optional fields are -1 (`clock_rate`, `large_tick_hits`, `slider_end_hits`,
/// `passed_objects`, `legacy_total_score`, `accuracy`, `max_combo`).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SppScore {
    pub mode: f64,
    pub mods: f64,
    pub lazer: f64,
    pub clock_rate: f64,
    pub max_combo: f64,
    pub n300: f64,
    pub n100: f64,
    pub n50: f64,
    pub n_geki: f64,
    pub n_katu: f64,
    pub n_miss: f64,
    pub large_tick_hits: f64,
    pub slider_end_hits: f64,
    pub passed_objects: f64,
    pub legacy_total_score: f64,
    pub accuracy: f64,
}

/// `SppPerf`. `calculator`: 0 official, 1 relax (osu_2019), 2 akatsuki; -1 = failed.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SppPerf {
    pub pp: f64,
    pub pp_aim: f64,
    pub pp_speed: f64,
    pub pp_acc: f64,
    pub pp_flashlight: f64,
    pub stars: f64,
    pub max_combo: f64,
    pub effective_miss_count: f64,
    pub calculator: f64,
}

/// `SppStats`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SppStats {
    pub ar: f64,
    pub od: f64,
    pub cs: f64,
    pub hp: f64,
    pub clock_rate: f64,
    pub n_objects: f64,
    pub mode: f64,
}

fn opt_u32(v: f64) -> Option<u32> {
    (v.is_finite() && v >= 0.0).then(|| v.min(u32::MAX as f64) as u32)
}

fn opt_f64(v: f64) -> Option<f64> {
    (v.is_finite() && v >= 0.0).then_some(v)
}

impl From<&SppScore> for Score {
    fn from(s: &SppScore) -> Self {
        let n = |v: f64| opt_u32(v).unwrap_or(0);
        Score {
            mode: n(s.mode).min(3) as u8,
            mods: n(s.mods),
            lazer: s.lazer > 0.0,
            clock_rate: opt_f64(s.clock_rate).filter(|&r| r > 0.0),
            max_combo: opt_u32(s.max_combo).unwrap_or(u32::MAX),
            n300: n(s.n300),
            n100: n(s.n100),
            n50: n(s.n50),
            n_geki: n(s.n_geki),
            n_katu: n(s.n_katu),
            n_miss: n(s.n_miss),
            large_tick_hits: opt_u32(s.large_tick_hits),
            slider_end_hits: opt_u32(s.slider_end_hits),
            passed_objects: opt_u32(s.passed_objects),
            legacy_total_score: opt_u32(s.legacy_total_score).filter(|&t| t > 0),
            accuracy: opt_f64(s.accuracy),
        }
    }
}

/// Parses a .osu file. Null when it can't be parsed or is too suspicious to calculate.
/// Free with `spp_map_free`.
///
/// # Safety
/// `bytes` must point to `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn spp_map_new(bytes: *const u8, len: usize) -> *mut Map {
    guarded(std::ptr::null_mut(), || {
        if bytes.is_null() || len == 0 {
            return std::ptr::null_mut();
        }
        match Map::from_bytes(std::slice::from_raw_parts(bytes, len)) {
            Ok(map) => Box::into_raw(Box::new(map)),
            Err(_) => std::ptr::null_mut(),
        }
    })
}

/// # Safety
/// `map` must come from `spp_map_new` and not be used after.
#[no_mangle]
pub unsafe extern "C" fn spp_map_free(map: *mut Map) {
    if !map.is_null() {
        guarded((), || drop(Box::from_raw(map)));
    }
}

/// The map's own mode (0-3), or -1.
///
/// # Safety
/// `map` from `spp_map_new` or null.
#[no_mangle]
pub unsafe extern "C" fn spp_map_mode(map: *const Map) -> i32 {
    map.as_ref().map_or(-1, |m| m.mode() as i32)
}

/// Hit objects in the map, or 0.
///
/// # Safety
/// `map` from `spp_map_new` or null.
#[no_mangle]
pub unsafe extern "C" fn spp_map_n_objects(map: *const Map) -> u32 {
    map.as_ref().map_or(0, Map::n_objects)
}

/// pp and stars for one score. Returns 0 on success, -1 on failure (`out` then zeroed, calculator -1).
///
/// # Safety
/// `map` from `spp_map_new`; `score` and `out` valid pointers.
#[no_mangle]
pub unsafe extern "C" fn spp_calculate(map: *mut Map, score: *const SppScore, out: *mut SppPerf) -> i32 {
    if out.is_null() {
        return -1;
    }
    let failed = SppPerf { calculator: -1.0, ..SppPerf::default() };
    let result = guarded(None, || {
        let (map, score) = (map.as_mut()?, score.as_ref()?);
        map.calculate(&Score::from(score)).ok()
    });
    match result {
        Some(p) => {
            *out = SppPerf {
                pp: p.pp,
                pp_aim: p.pp_aim,
                pp_speed: p.pp_speed,
                pp_acc: p.pp_acc,
                pp_flashlight: p.pp_flashlight,
                stars: p.stars,
                max_combo: p.max_combo as f64,
                effective_miss_count: p.effective_miss_count,
                calculator: p.calculator.map_or(-1.0, |c| c as u8 as f64),
            };
            0
        }
        None => {
            *out = failed;
            -1
        }
    }
}

/// Map stats with the score's mods/rate applied (only mode, mods, lazer, clock_rate are read).
/// 0 on success, -1 on failure.
///
/// # Safety
/// `map` from `spp_map_new`; `score` and `out` valid pointers.
#[no_mangle]
pub unsafe extern "C" fn spp_stats(map: *const Map, score: *const SppScore, out: *mut SppStats) -> i32 {
    if out.is_null() {
        return -1;
    }
    let result = guarded(None, || map.as_ref()?.stats(&Score::from(score.as_ref()?)).ok());
    match result {
        Some(s) => {
            *out = SppStats {
                ar: s.ar,
                od: s.od,
                cs: s.cs,
                hp: s.hp,
                clock_rate: s.clock_rate,
                n_objects: s.n_objects as f64,
                mode: s.mode as f64,
            };
            0
        }
        None => {
            *out = SppStats::default();
            -1
        }
    }
}

/// Strain graph for the score's mods. Writes up to `cap` values to `out` and the section length in
/// ms to `section_len_ms`; returns how many values there are in all (call again with a bigger
/// buffer if that's more than `cap`), or -1 on failure.
///
/// # Safety
/// `map` from `spp_map_new`; `score` valid; `out` has room for `cap` f64s (may be null if cap 0).
#[no_mangle]
pub unsafe extern "C" fn spp_strains(
    map: *const Map,
    score: *const SppScore,
    out: *mut f64,
    cap: usize,
    section_len_ms: *mut f64,
) -> i64 {
    let result = guarded(None, || map.as_ref()?.strains(&Score::from(score.as_ref()?)).ok());
    let Some(graph) = result else { return -1 };
    if !section_len_ms.is_null() {
        *section_len_ms = graph.section_len_ms;
    }
    if !out.is_null() {
        let n = graph.values.len().min(cap);
        std::ptr::copy_nonoverlapping(graph.values.as_ptr(), out, n);
    }
    graph.values.len() as i64
}

/// Which calculator pays a play: 0 official, 1 relax (osu_2019), 2 akatsuki.
#[no_mangle]
pub extern "C" fn spp_route(mode: u32, mods: u32) -> i32 {
    crate::Calculator::route(mode.min(255) as u8, mods) as i32
}

/// Static NUL-terminated version string.
#[no_mangle]
pub extern "C" fn spp_version() -> *const u8 {
    crate::VERSION.as_ptr()
}

/// The `SppScore` field count, so a host can check it was built against the same layout.
#[no_mangle]
pub extern "C" fn spp_abi_score_fields() -> u32 {
    (std::mem::size_of::<SppScore>() / 8) as u32
}

/// WebAssembly hosts: memory for the .osu bytes and the structs.
#[no_mangle]
pub extern "C" fn spp_alloc(len: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len.max(1));
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// # Safety
/// `ptr`/`len` exactly as from `spp_alloc`.
#[no_mangle]
pub unsafe extern "C" fn spp_free_bytes(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        drop(Vec::from_raw_parts(ptr, 0, len.max(1)));
    }
}
