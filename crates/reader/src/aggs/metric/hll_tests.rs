//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use lucene_aggs::cardinality_sketch::{Sketch, mix64};

use super::*;

/// What OpenSearch 3.10.0's `HyperLogLogPlusPlus` answers, a line per
/// precision: `n=a/b`, where `a` is the cardinality of `mix64(0..n)` and `b`
/// that of `mix64(7v)` for `v` in `0..n`, collected into two sketches by
/// `v % 3 == 0` and merged into an empty one.
const JAVA: &str = "
p4: 1=1/1 2=2/2 3=3/3 4=3/5 5=5/6 10=12/12 50=41/44 100=116/82 191=168/263 192=168/263 193=168/263 200=168/263 1000=1568/949 3071=4404/2757 3072=4404/2757 3073=4404/2757 5000=6070/5114 20000=12948/16119 100000=99453/127403 300000=255707/292325
p5: 1=1/1 2=2/2 3=3/3 4=4/4 5=5/5 10=11/9 50=46/51 100=104/91 191=162/212 192=162/212 193=162/212 200=171/216 1000=1196/1117 3071=3902/3133 3072=3902/3133 3073=3902/3133 5000=6074/5485 20000=16441/17604 100000=119305/95605 300000=247732/211412
p6: 1=1/1 2=2/2 3=3/3 4=4/4 5=5/5 10=10/10 50=45/51 100=88/90 191=191/201 192=194/201 193=194/201 200=206/202 1000=1102/1042 3071=3575/3496 3072=3575/3496 3073=3575/3496 5000=6473/5903 20000=17713/17317 100000=112628/109507 300000=264778/253503
p7: 1=1/1 2=2/2 3=3/3 4=4/4 5=5/5 10=10/10 50=47/48 100=93/94 191=191/184 192=192/184 193=192/184 200=206/188 1000=1034/1087 3071=3306/3443 3072=3306/3443 3073=3306/3443 5000=6149/5793 20000=18286/18267 100000=91728/94575 300000=277883/254199
p8: 1=1/1 2=2/2 3=3/3 4=4/4 5=5/5 10=10/10 50=48/52 100=96/99 191=190/196 192=192/196 193=192/196 200=200/198 1000=1018/1078 3071=3206/3441 3072=3206/3441 3073=3206/3441 5000=5795/5359 20000=18727/18455 100000=94648/95029 300000=277764/285733
p9: 1=1/1 2=2/2 3=3/3 4=4/4 5=5/5 10=10/10 50=50/50 100=100/99 191=201/195 192=202/195 193=204/195 200=211/198 1000=1039/1053 3071=3032/3225 3072=3032/3225 3073=3032/3225 5000=5289/5217 20000=18576/20106 100000=99176/100244 300000=296986/304025
p10: 1=1/1 2=2/2 3=3/3 4=4/4 5=5/5 10=10/10 50=50/50 100=100/100 191=191/191 192=192/192 193=199/198 200=206/200 1000=1025/1039 3071=3055/3129 3072=3058/3135 3073=3063/3142 5000=5159/5183 20000=19969/20676 100000=103342/104664 300000=296317/315870
p11: 1=1/1 2=2/2 3=3/3 4=4/4 5=5/5 10=10/10 50=50/50 100=100/100 191=191/191 192=192/192 193=193/193 200=200/200 1000=1003/1008 3071=3043/3047 3072=3041/3047 3073=3060/3046 5000=5041/4995 20000=19801/20626 100000=101910/102213 300000=299695/302656
p12: 1=1/1 2=2/2 3=3/3 4=4/4 5=5/5 10=10/10 50=50/50 100=100/100 191=191/191 192=192/192 193=193/193 200=200/200 1000=1002/1012 3071=3017/3082 3072=3019/3084 3073=3021/3086 5000=4926/4942 20000=19620/19970 100000=98918/99833 300000=299099/299168
p13: 1=1/1 2=2/2 3=3/3 4=4/4 5=5/5 10=10/10 50=50/50 100=100/100 191=191/191 192=192/192 193=193/193 200=200/200 1000=1000/1000 3071=3035/3086 3072=3036/3087 3073=3038/3088 5000=4973/4968 20000=19889/19778 100000=99158/99323 300000=299148/295301
p14: 1=1/1 2=2/2 3=3/3 4=4/4 5=5/5 10=10/10 50=50/50 100=100/100 191=191/191 192=192/192 193=193/193 200=200/200 1000=1000/1000 3071=3071/3071 3072=3072/3072 3073=3057/3068 5000=4973/4985 20000=19798/19822 100000=99776/99515 300000=300703/297688
p15: 1=1/1 2=2/2 3=3/3 4=4/4 5=5/5 10=10/10 50=50/50 100=100/100 191=191/191 192=192/192 193=193/193 200=200/200 1000=1000/1000 3071=3071/3071 3072=3072/3072 3073=3073/3073 5000=5000/5000 20000=19953/19715 100000=100238/99871 300000=299214/297502
p16: 1=1/1 2=2/2 3=3/3 4=4/4 5=5/5 10=10/10 50=50/50 100=100/100 191=191/191 192=192/192 193=193/193 200=200/200 1000=1000/1000 3071=3071/3071 3072=3072/3072 3073=3073/3073 5000=5000/5000 20000=20048/19998 100000=99937/99722 300000=299170/299889
p17: 1=1/1 2=2/2 3=3/3 4=4/4 5=5/5 10=10/10 50=50/50 100=100/100 191=191/191 192=192/192 193=193/193 200=200/200 1000=1000/1000 3071=3071/3071 3072=3072/3072 3073=3073/3073 5000=5000/5000 20000=19999/19999 100000=100137/100264 300000=299629/300694
p18: 1=1/1 2=2/2 3=3/3 4=4/4 5=5/5 10=10/10 50=50/50 100=100/100 191=191/191 192=192/192 193=193/193 200=200/200 1000=1000/1000 3071=3071/3071 3072=3072/3072 3073=3073/3073 5000=5000/5000 20000=19999/19999 100000=99995/100170 300000=299400/301237
";

/// The bytes lucene-aggs writes for `values`' hashes at `p`.
fn written(p: u32, values: impl Iterator<Item = i64>) -> Vec<u8> {
    let mut sketch = Sketch::new(p).expect("precision");
    for v in values {
        sketch.collect(mix64(v));
    }
    let mut out = Vec::new();
    sketch.write_to(&mut out);
    out
}

fn read(bytes: &[u8]) -> Hll {
    Hll::read(bytes).expect("reads")
}

#[test]
fn estimates_and_merges_match_opensearch() {
    let mut checked = 0;
    for line in JAVA.lines().filter(|l| !l.is_empty()) {
        let (p, cases) = line.split_once(':').expect("p:");
        let p: u32 = p.trim_start_matches('p').parse().expect("p");
        for case in cases.split_whitespace() {
            let (n, expect) = case.split_once('=').expect("n=");
            let (one, merged) = expect.split_once('/').expect("a/b");
            let n: i64 = n.parse().expect("n");
            let sketch = read(&written(p, 0..n));
            assert_eq!(sketch.precision(), p);
            assert_eq!(sketch.cardinality().to_string(), one, "p{p} n{n}");
            let x = read(&written(p, (0..n).filter(|v| v % 3 == 0).map(|v| v * 7)));
            let y = read(&written(p, (0..n).filter(|v| v % 3 != 0).map(|v| v * 7)));
            let mut reduced = Hll::empty(p);
            assert!(reduced.merge(&x));
            assert!(reduced.merge(&y));
            assert_eq!(
                reduced.cardinality().to_string(),
                merged,
                "p{p} n{n} merged"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 15 * 20);
}

#[test]
fn sketches_of_another_precision_do_not_merge() {
    let mut a = read(&written(4, 0..10));
    let before = a.clone();
    assert!(!a.merge(&read(&written(5, 0..10))));
    assert_eq!(a, before);
}

#[test]
fn bytes_lucene_aggs_cannot_have_written_are_refused() {
    let good = written(4, 0..2);
    assert!(Hll::read(&good).is_some());
    assert!(Hll::read(&[]).is_none());
    assert!(Hll::read(&[3, 0, 0]).is_none());
    assert!(Hll::read(&[19, 0, 0]).is_none());
    assert!(Hll::read(&[4]).is_none());
    assert!(Hll::read(&[4, 2]).is_none());
    // A count the hashes do not fill, an empty slot, a truncated vlong.
    assert!(Hll::read(&[4, 0, 2, 0, 0, 0, 1]).is_none());
    assert!(Hll::read(&[4, 0, 1, 0, 0, 0, 0]).is_none());
    assert!(Hll::read(&[4, 0, 0x80]).is_none());
    // Registers: exactly 2^p of them.
    let mut registers = vec![4u8, 1];
    registers.extend([0u8; 16]);
    assert_eq!(read(&registers).cardinality(), 0);
    registers.push(0);
    assert!(Hll::read(&registers).is_none());
}

#[test]
fn a_vlong_reads_seven_bits_a_byte() {
    assert_eq!(read_vlong(&[0x05, 9]), Some((5, &[9u8][..])));
    assert_eq!(read_vlong(&[0x81, 0x01]), Some((129, &[][..])));
    assert_eq!(read_vlong(&[0xff; 11]), None);
}

#[test]
fn precision_follows_the_threshold_as_opensearch_computes_it() {
    // Printed by OpenSearch 3.10.0's `precisionFromThreshold`.
    for (count, p) in [
        (0, 4),
        (1, 4),
        (2, 4),
        (3, 5),
        (10, 6),
        (100, 10),
        (1000, 13),
        (3000, 14),
        (40000, 18),
        (1 << 40, 18),
        (4_611_686_018_427_387_904, 18),
    ] {
        assert_eq!(precision_from_threshold(count), Ok(p), "{count}");
    }
    for (count, got) in [
        (1_730_000_000_000_000_000, "-9220076800506331136"),
        (2_305_843_009_213_693_951, "-6148914324732641280"),
        (2_305_843_009_213_693_952, "-6148914324732641280"),
        (i64::MAX, "-4"),
    ] {
        assert_eq!(
            precision_from_threshold(count),
            Err(format!("maxValue must be non-negative (got: {got})")),
            "{count}"
        );
    }
}

#[test]
fn a_sketch_reserves_five_times_its_registers() {
    assert_eq!(sketch_bytes(4), 96);
    assert_eq!(sketch_bytes(14), (5 << 14) + 16);
    assert_eq!(sketch_bytes(18), (5 << 18) + 16);
    assert_eq!(sketch_bytes(30), (5 << 18) + 16);
}

/// The sketches OpenSearch 3.10.0's `InternalCardinality` wrote
/// (`fixtures/search-shard/java-cardinality-3.10.0.txt`) read and write back
/// to the same bytes.
#[test]
fn a_sketch_writes_the_bytes_java_wrote() {
    const RECORDED: &str =
        include_str!("../../../../../fixtures/search-shard/java-cardinality-3.10.0.txt");
    // The answer's bytes before the sketch: the list, the name, no metadata,
    // `raw`, and the present flag.
    let prefix = "010b63617264696e616c6974790175ff0372617701";
    let mut seen = 0;
    for line in RECORDED.lines() {
        let Some((name, hex)) = line.split_once(' ') else {
            continue;
        };
        if !name.starts_with("response.cardinality_") {
            continue;
        }
        let sketch = hex.strip_prefix(prefix).expect("a present sketch");
        let bytes: Vec<u8> = (0..sketch.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&sketch[i..i + 2], 16).expect("hex"))
            .collect();
        let hll = Hll::read(&bytes).expect("reads");
        assert_eq!(hll.to_bytes(), bytes, "{name}");
        seen += 1;
    }
    assert_eq!(seen, 3);
}
