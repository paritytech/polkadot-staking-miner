use substrate_wasm_builder::WasmBuilder;

pub const NOMINATORS: Option<&str> = option_env!("N");
pub const CANDIDATES: Option<&str> = option_env!("C");
pub const VALIDATORS: Option<&str> = option_env!("V");

fn main() {
	WasmBuilder::new()
		.with_current_project()
		.export_heap_base()
		.import_memory()
		.build()
}
