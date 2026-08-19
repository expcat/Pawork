fn main() -> Result<(), Box<dyn std::error::Error>> {
    let check_only = match std::env::args().nth(1).as_deref() {
        None => false,
        Some("--check") => true,
        Some(argument) => return Err(format!("unknown argument: {argument}").into()),
    };

    pawork_protocol::typegen::run(check_only)?;
    if check_only {
        println!("TypeScript declarations are up to date");
    } else {
        println!("TypeScript declarations generated under schemas/");
    }
    Ok(())
}
