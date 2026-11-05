//! The Rust generators make exactly the programs validation/fuzz.py made (fuzz_generators.json, written from the
//! venv's CPython 3.9.6 before the Python toolchain was deleted): 300 loop and 200 handler programs per seed, each
//! followed by its job seed, as fuzz.jobs() draws them.
use kern::pyrandom::Random;
use kern_validation::fuzz::{gen_handler, gen_loop};
use serde_json::Value;

#[test]
fn generators_match_fuzz_py() {
    let v: Value = serde_json::from_str(include_str!("fuzz_generators.json")).unwrap();
    for (seed, progs) in v["seeds"].as_object().unwrap() {
        let mut rnd = Random::from_int(seed.parse().unwrap());
        for (kind, gen) in [("loops", gen_loop as fn(&mut Random) -> String), ("handlers", gen_handler)] {
            for (i, p) in progs[kind].as_array().unwrap().iter().enumerate() {
                let src = gen(&mut rnd);
                assert_eq!(src, p[0].as_str().unwrap(), "seed {seed}, {kind} #{i}");
                assert_eq!(rnd.randrange(0, 1_000_000_000), p[1].as_i64().unwrap(), "seed {seed}, {kind} #{i}: job seed");
            }
        }
    }
}
