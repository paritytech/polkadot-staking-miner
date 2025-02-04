use cfg_aliases::cfg_aliases;

fn main() {
	// Setup cfg aliases
	cfg_aliases! {
		legacy : { all(feature = "legacy", not(feature = "experimental-multi-block")) },
		experimental_multi_block : { all(feature = "experimental-multi-block", not(feature = "legacy")) },
	}
}
