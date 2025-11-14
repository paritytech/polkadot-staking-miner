#[test]
fn cli_version_works() {
	let output = assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!(env!("CARGO_PKG_NAME")))
		.arg("--version")
		.output()
		.unwrap();

	assert!(output.status.success(), "command returned with non-success exit code");
	let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();

	assert_eq!(version, format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")));
}
