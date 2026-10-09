// sqlx::migrate! embeds the migrations directory at compile time. Rebuild when
// a migration is added, not only when an existing file changes.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
