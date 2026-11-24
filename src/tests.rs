use super::*;

const MAP: &[u8] = include_bytes!("../tests/maps/2964220.osu");

fn fc(mods: u32) -> Score {
    Score { mods, accuracy: Some(100.0), ..Score::default() }
}

#[test]
fn routing_matches_the_server() {
    use mods::*;
    assert_eq!(Calculator::route(0, 0), Calculator::Official);
    assert_eq!(Calculator::route(0, HIDDEN | DOUBLE_TIME), Calculator::Official);
    assert_eq!(Calculator::route(0, RELAX), Calculator::Relax2019);
    assert_eq!(Calculator::route(0, RELAX | HIDDEN), Calculator::Relax2019);
    assert_eq!(Calculator::route(0, AUTOPILOT), Calculator::Akatsuki);
    assert_eq!(Calculator::route(1, RELAX), Calculator::Akatsuki);
    assert_eq!(Calculator::route(2, RELAX), Calculator::Akatsuki);
    assert_eq!(Calculator::route(3, 0), Calculator::Official);
}

#[test]
fn each_route_pays_something_sane() {
    let mut map = Map::from_bytes(MAP).unwrap();
    for m in [0, mods::RELAX, mods::AUTOPILOT, mods::HIDDEN | mods::DOUBLE_TIME | mods::RELAX] {
        let p = map.calculate(&fc(m)).unwrap();
        assert!(p.pp > 10.0 && p.pp < 3000.0, "mods {m}: {p:?}");
        assert!(p.stars > 1.0 && p.stars < 15.0, "mods {m}: {p:?}");
        assert_eq!(p.calculator, Some(Calculator::route(0, m)));
    }
}

#[test]
fn relax_is_not_vanilla() {
    let mut map = Map::from_bytes(MAP).unwrap();
    let nm = map.calculate(&fc(0)).unwrap();
    let rx = map.calculate(&fc(mods::RELAX)).unwrap();
    assert_ne!(nm.pp, rx.pp);
}

#[test]
fn partial_relax_play_stays_finite() {
    // patcher 1.5.2: a short partial Relax play once ran pp to inf.
    let mut map = Map::from_bytes(MAP).unwrap();
    for passed in [1, 2, 5, 20] {
        let s = Score { mods: mods::RELAX, passed_objects: Some(passed), n300: passed, max_combo: passed, ..Score::default() };
        let p = map.calculate(&s).unwrap();
        assert!(p.pp.is_finite() && p.pp < 3000.0, "{passed}: {p:?}");
    }
}

#[test]
fn hit_counts_beyond_the_map_are_clamped_for_relax() {
    let mut map = Map::from_bytes(MAP).unwrap();
    let s = Score { mods: mods::RELAX, n300: 1_000_000, ..Score::default() };
    let p = map.calculate(&s).unwrap();
    let full = map.calculate(&fc(mods::RELAX)).unwrap();
    assert!((p.pp - full.pp).abs() < 1e-6, "{p:?} vs {full:?}");
}

#[test]
fn misses_cost_pp() {
    let mut map = Map::from_bytes(MAP).unwrap();
    for m in [0, mods::RELAX, mods::AUTOPILOT] {
        let clean = map.calculate(&fc(m)).unwrap().pp;
        let missed = map.calculate(&Score { n_miss: 5, ..fc(m) }).unwrap().pp;
        assert!(missed < clean, "mods {m}: {missed} !< {clean}");
    }
}

#[test]
fn custom_rate_only_off_the_usual_speed() {
    use mods::*;
    assert_eq!(custom_rate(DOUBLE_TIME, Some(1.2)), Some(1.2));
    assert_eq!(custom_rate(DOUBLE_TIME, Some(1.5)), None);
    assert_eq!(custom_rate(HALF_TIME, Some(0.6)), Some(0.6));
    assert_eq!(custom_rate(HARD_ROCK, Some(1.2)), None);
    assert_eq!(custom_rate(DOUBLE_TIME, Some(9.0)), Some(2.0));
}

#[test]
fn converts_and_stats() {
    let mut map = Map::from_bytes(MAP).unwrap();
    for mode in 1..=3 {
        let p = map.calculate(&Score { mode, ..fc(0) }).unwrap();
        assert!(p.pp > 0.0, "mode {mode}: {p:?}");
    }
    let nm = map.stats(&fc(0)).unwrap();
    let dt = map.stats(&fc(mods::DOUBLE_TIME)).unwrap();
    assert!(dt.ar > nm.ar && (dt.clock_rate - 1.5).abs() < 1e-9);
    let g = map.strains(&fc(0)).unwrap();
    assert!(g.values.len() > 10 && g.section_len_ms > 0.0);
}

#[test]
fn garbage_is_an_error_not_a_panic() {
    assert!(Map::from_bytes(b"not a beatmap").is_err() || Map::from_bytes(b"not a beatmap").unwrap().n_objects() == 0);
    unsafe {
        let m = ffi::spp_map_new(std::ptr::null(), 0);
        assert!(m.is_null());
        let mut out = ffi::SppPerf::default();
        assert_eq!(ffi::spp_calculate(std::ptr::null_mut(), std::ptr::null(), &mut out), -1);
        assert_eq!(out.calculator, -1.0);
    }
}

#[test]
fn ffi_round_trip() {
    unsafe {
        let m = ffi::spp_map_new(MAP.as_ptr(), MAP.len());
        assert!(!m.is_null());
        let s = ffi::SppScore {
            mode: 0.0, mods: mods::RELAX as f64, lazer: 0.0, clock_rate: -1.0, max_combo: -1.0,
            n300: 0.0, n100: 0.0, n50: 0.0, n_geki: 0.0, n_katu: 0.0, n_miss: 0.0,
            large_tick_hits: -1.0, slider_end_hits: -1.0, passed_objects: -1.0,
            legacy_total_score: -1.0, accuracy: 100.0,
        };
        let mut out = ffi::SppPerf::default();
        assert_eq!(ffi::spp_calculate(m, &s, &mut out), 0);
        assert_eq!(out.calculator, 1.0);
        let rust = Map::from_bytes(MAP).unwrap().calculate(&fc(mods::RELAX)).unwrap();
        assert_eq!(out.pp, rust.pp);
        assert_eq!(ffi::spp_abi_score_fields(), 16);
        ffi::spp_map_free(m);
    }
}
