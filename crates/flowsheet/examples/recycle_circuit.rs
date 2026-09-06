//! Builds the demo recycle circuit and prints its wiring.
//!
//! Run with:  cargo run --example recycle_circuit

use flowsheet::demo::build_flowsheet;

fn main() {
    let flowsheet = build_flowsheet();
    println!("{flowsheet:#?}");
}
