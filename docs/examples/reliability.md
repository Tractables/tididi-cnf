# Network reliability

Two sites, s and t, are joined by a small network of five links. Each link
works on a given day with probability 0.9, independently of the others. How
likely is it that s can still reach t, and which link matters most?

Run the complete program with `cargo run --example reliability`.

## Describe connection as clauses

Links 1 (s–a), 2 (s–b), 3 (a–b), 4 (a–t) and 5 (b–t) form the classic bridge
network. A variable is true when its link works. The sites are disconnected
exactly when every link of some **cut** fails, so they are connected when
every cut keeps a working link: {1, 2} around s, {4, 5} around t, and the two
cuts through the bridge, {1, 3, 5} and {2, 3, 4}. Each cut becomes one clause.

```rust,ignore,{class=tested-example}
let network = "\
c t wmc
p cnf 5 4
1 2 0
4 5 0
1 3 5 0
2 3 4 0
";
```

The `c t wmc` line declares the task: a weighted model count.

## Weigh each link

A weighted model count gives each literal a weight. An assignment weighs the
product of its literals' weights, and the count is the sum over the
assignments that satisfy the formula. With a link's probability of working on
its positive literal and the probability of failing on its negative one, an
assignment weighs its probability, and the count is the probability that the
sites are connected. `c p weight` lines carry the weights; a literal without
one weighs 1, so both literals of each link get a line.

```rust,ignore,{class=tested-example}
/// The weight lines for links that each work with the given percentage.
fn weights(percent_working: [u32; 5]) -> String {
    let mut lines = String::new();
    for (link, percent) in (1..).zip(percent_working) {
        lines += &format!("c p weight {link} {percent}/100 0\n");
        lines += &format!("c p weight -{link} {}/100 0\n", 100 - percent);
    }
    lines
}
```

## Count

The header declares the task, so [`count`](crate::count) needs no
[`Mode`](crate::Mode). The weighted count is computed in floating point, and
[`Count::to_f64`](crate::Count::to_f64) reads it as a number.

```rust,ignore,{class=tested-example}
let reliable = [90; 5];
let instance = format!("{network}{}", weights(reliable));
let answer = tididi_bonsai::count(parse_dimacs(instance.as_bytes())?, None, &Limits::default())?;
println!("Connected: {:.5}", answer.count().to_f64());
```

Output:

```text
Connected: 0.97848
```

## Take one link down

A link that is down works with probability 0. Taking each link down in turn
shows how much the network depends on it:

```rust,ignore,{class=tested-example}
for link in 1..=5 {
    let mut broken = reliable;
    broken[link - 1] = 0;
    let instance = format!("{network}{}", weights(broken));
    let answer = tididi_bonsai::count(parse_dimacs(instance.as_bytes())?, None, &Limits::default())?;
    println!("Connected with link {link} down: {:.5}", answer.count().to_f64());
}
```

Output:

```text
Connected with link 1 down: 0.88290
Connected with link 2 down: 0.88290
Connected with link 3 down: 0.96390
Connected with link 4 down: 0.88290
Connected with link 5 down: 0.88290
```

Without the bridge, s reaches t over a–t or b–t: 1 − (1 − 0.81)² = 0.9639.
Each outer link matters more: without link 1, every route starts on link 2.

The [complete program](https://github.com/Tractables/tididi-bonsai/blob/v0.1.0/examples/reliability.rs)
also builds the instance text for each case.
