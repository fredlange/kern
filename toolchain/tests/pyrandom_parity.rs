//! Replays pyrandom_vectors.json (written by validation/pyrandom_vectors.py from CPython 3.9.6) and demands the same results.
use kern::pyrandom::Random;
use serde_json::Value;

#[test]
fn every_call_matches_cpython() {
    let v: Value = serde_json::from_str(include_str!("pyrandom_vectors.json")).unwrap();
    assert_eq!(v["python"], "3.9.6");
    let mut n = 0;
    for case in v["cases"].as_array().unwrap() {
        let seed = case["seed"].as_str().unwrap();
        let mut r = if case["kind"] == "str" { Random::from_str(seed) } else { Random::from_int(seed.parse().unwrap()) };
        for op in case["ops"].as_array().unwrap() {
            let o = op.as_array().unwrap();
            let i = |k: usize| o[k].as_i64().unwrap();
            let ctx = format!("seed {seed:?}, call {n}: {o:?}");
            match o[0].as_str().unwrap() {
                "randint" => assert_eq!(r.randint(i(1), i(2)), i(3), "{ctx}"),
                "randrange" => assert_eq!(r.randrange(i(1), i(2)), i(3), "{ctx}"),
                "random" => assert_eq!(r.random().to_bits(), o[1].as_f64().unwrap().to_bits(), "{ctx}"),
                "choice" => {
                    let seq: Vec<i64> = (0..i(1)).collect();
                    assert_eq!(*r.choice(&seq), i(2), "{ctx}")
                }
                "shuffle" => {
                    let mut x: Vec<i64> = (0..i(1)).collect();
                    r.shuffle(&mut x);
                    let want: Vec<i64> = o[2].as_array().unwrap().iter().map(|e| e.as_i64().unwrap()).collect();
                    assert_eq!(x, want, "{ctx}")
                }
                "getrandbits" => assert_eq!(r.getrandbits(i(1) as u32).to_string(), o[2].as_str().unwrap(), "{ctx}"),
                other => panic!("unknown op {other}"),
            }
            n += 1;
        }
    }
    assert_eq!(n, 6000);
}
