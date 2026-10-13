//! The [worked examples](examples) put the crate to work on applications, from
//! counting a product's configurations to the distinct outputs of a circuit.
//! The [architecture reference](architecture) maps the source for
//! contributors. The command-line guide, the input format and the run record
//! are documented with the
//! [binary](https://github.com/Tractables/tididi-bonsai/tree/main/docs).

// Keep the index, sidebar and module declarations in one reading order.
macro_rules! walkthroughs {
    ($($name:ident => ($title:literal, $description:literal)),+ $(,)?) => {
        macro_rules! navigation {
            () => {
                concat!(
                    "<script>(() => { const examples = [",
                    $("[\"", stringify!($name), "\",\"", $title, "\"],",)+
                    "];", include_str!("../docs/example-navigation.js"), "})();</script>"
                )
            };
        }

        #[doc = concat!(
            "Worked examples, from counting to compiling on a vtree of your choice.\n\n",
            $("1. [", $title, "](examples::", stringify!($name), ") — ", $description, "\n",)+
        )]
        #[doc = navigation!()]
        pub mod examples {
            $(
                #[doc = include_str!(concat!("../docs/examples/", stringify!($name), ".md"))]
                // Only excerpts checked against runnable programs suppress the badge.
                #[doc = "<style>.example-wrap.ignore:has(> pre.tested-example) > .tooltip { display: none; }</style>"]
                #[doc = navigation!()]
                pub mod $name {}
            )+
        }
    };
}

walkthroughs! {
    catalog => ("Product configurations", "Count the configurations rules allow, project them, and keep the circuit."),
    reliability => ("Network reliability", "Weigh assignments by probabilities to find how likely two sites stay connected."),
    circuits => ("Distinct outputs", "Encode a multiplier as clauses and count the products it outputs."),
    vtree_choice => ("Choosing a vtree", "Compile one formula on several vtrees and compare the circuits."),
}

#[doc = include_str!("../docs/architecture.md")]
pub mod architecture {}
