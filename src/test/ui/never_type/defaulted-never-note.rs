// We need to opt into the `never_type_fallback` feature
// to trigger the requirement that this is testing.
#![feature(never_type, never_type_fallback)]

#![allow(unused)]

trait Deserialize: Sized {
    fn deserialize() -> Result<Self, String>;
}

impl Deserialize for () {
    fn deserialize() -> Result<(), String> {
        Ok(())
    }
}

trait ImplementedForUnitButNotNever {}

impl ImplementedForUnitButNotNever for () {}

fn foo<T: ImplementedForUnitButNotNever>(_t: T) {}
//~^ NOTE required by this bound in `foo`
//~| NOTE required by a bound in `foo`
fn smeg() {
    let _x = return;
    foo(_x);
    //~^ ERROR the trait bound
    //~| NOTE the trait `ImplementedForUnitButNotNever` is not implemented
    //~| NOTE this trait is implemented for `()`
    //~| NOTE this error might have been caused
    //~| HELP did you intend
}

fn main() {
    smeg();
}
