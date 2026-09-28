// A bare name in `provide!` is a type's own declaration. A unit value written as a bare name is
// refused, naming `value` as the spelling for a value.

struct Unit;

ulo::key!(Slot: Unit);

fn main() {
    let _ = ulo::provide!(Slot => Unit);
}
