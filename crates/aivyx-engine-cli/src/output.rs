/// Print a section header.
pub fn header(title: &str) {
    println!("\n  {title}");
    println!("  {}", "─".repeat(title.len()));
}

/// Print a key-value pair with alignment.
pub fn kv(key: &str, value: &str) {
    println!("  {key:<30} {value}");
}

/// Print a success message.
pub fn success(msg: &str) {
    println!("  [ok] {msg}");
}

/// Print an error message.
pub fn error(msg: &str) {
    eprintln!("  [error] {msg}");
}
