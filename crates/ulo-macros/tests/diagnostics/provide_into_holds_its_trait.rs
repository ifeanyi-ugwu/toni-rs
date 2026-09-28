// A contribution to the collection of a trait object must implement the trait.

trait Plugin: Send + Sync {}

struct NotAPlugin;

fn main() {
    let _ = ulo::provide!(into dyn Plugin => value NotAPlugin);
}
