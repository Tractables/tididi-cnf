//! The probability that a network whose links fail at random still connects
//! two sites, as a weighted model count.
//! Run with `cargo run --example reliability`.

fn main() -> Result<(), tididi_bonsai::Error> {
    use tididi_bonsai::{parse_dimacs, Limits};

    // Links 1 (s–a), 2 (s–b), 3 (a–b), 4 (a–t) and 5 (b–t): s reaches t
    // unless every link of a cut fails, so every cut keeps a working link.
    let network = "\
c t wmc
p cnf 5 4
1 2 0
4 5 0
1 3 5 0
2 3 4 0
";

    /// The weight lines for links that each work with the given percentage.
    fn weights(percent_working: [u32; 5]) -> String {
        let mut lines = String::new();
        for (link, percent) in (1..).zip(percent_working) {
            lines += &format!("c p weight {link} {percent}/100 0\n");
            lines += &format!("c p weight -{link} {}/100 0\n", 100 - percent);
        }
        lines
    }

    let reliable = [90; 5];
    let instance = format!("{network}{}", weights(reliable));
    let answer = tididi_bonsai::count(parse_dimacs(instance.as_bytes())?, None, &Limits::default())?;
    println!("Connected: {:.5}", answer.count().to_f64());

    for link in 1..=5 {
        let mut broken = reliable;
        broken[link - 1] = 0;
        let instance = format!("{network}{}", weights(broken));
        let answer = tididi_bonsai::count(parse_dimacs(instance.as_bytes())?, None, &Limits::default())?;
        println!("Connected with link {link} down: {:.5}", answer.count().to_f64());
    }
    Ok(())
}
