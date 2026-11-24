//! `cargo run --example pp -- <map.osu> <mode> <mods> <n300> <n100> <n50> <miss> <combo>`: one score's pp
//! as JSON. Used by `scripts/parity.sh` to compare against the server's pp-sidecar.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let n = |i: usize| a[i].parse::<u32>().unwrap();
    let bytes = std::fs::read(&a[0]).unwrap();
    let mut map = somtum_pp::Map::from_bytes(&bytes).unwrap();
    let s = somtum_pp::Score {
        mode: n(1) as u8, mods: n(2), n300: n(3), n100: n(4), n50: n(5), n_miss: n(6), max_combo: n(7),
        ..Default::default()
    };
    let p = map.calculate(&s).unwrap();
    println!("{{\"pp\":{},\"starRating\":{},\"calculator\":\"{}\"}}", p.pp, p.stars, p.calculator.unwrap().name());
}
