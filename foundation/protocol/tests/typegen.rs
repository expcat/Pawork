//! typegen 输出必须与检入的 `schemas/` 一致。

#[test]
fn typegen_matches_checked_in_schemas() {
    pawork_protocol::typegen::check().expect(
        "schemas/ is stale; run `cargo run -p pawork-protocol --features typegen --bin pawork-protocol-typegen`",
    );
}
