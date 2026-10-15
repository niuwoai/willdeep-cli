use std::{env, fs};
use willdeep_core::rsi::contract;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let paths: Vec<_> = env::args().skip(1).collect();
    assert!(paths.len() >= 3);
    let candidate = contract::parse_json(&fs::read(&paths[0])?)?;
    let spec = contract::parse_json(&fs::read(&paths[1])?)?;
    contract::validate_candidate(&candidate)?;
    contract::validate(&spec)?;
    assert_eq!(candidate["candidate_id"], spec["candidate_id"]);
    assert_eq!(candidate["grant_id"], spec["grant_id"]);
    assert_eq!(
        candidate["baseline_package"]["definition"]["definition_id"],
        spec["definition_id"]
    );
    assert_eq!(
        candidate["baseline_package"]["sha256"],
        spec["baseline_revision"]
    );
    assert_eq!(
        candidate["candidate_package"]["sha256"],
        spec["candidate_revision"]
    );
    assert_eq!(
        contract::hash(&candidate["baseline_package"]["definition"]["skills"])?,
        spec["conditions"]["skills_sha256"]
    );
    for path in &paths[2..] {
        let evidence = contract::parse_json(&fs::read(path)?)?;
        contract::bind_evidence(&spec, &evidence)?;
    }
    println!(
        "Rust contract validates candidate, experiment and {} peer evidence rows",
        paths.len() - 2
    );
    Ok(())
}
