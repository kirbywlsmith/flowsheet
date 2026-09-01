//! Flowsheet graph functions.

use crate::flowsheet::{Flowsheet, UnitId};

impl Flowsheet {
    /// Groups the flowsheet's units into topologically ordered evaluation waves.
    ///
    /// Wave `i` holds every unit whose inlets are all evaluated at wave `i - 1`.
    /// A wave's units are independent - they can be solved in parallel.
    ///
    /// # Errors
    ///
    /// Returns the units that could not be ordered - those inside a cycle or downstream of one.
    pub fn evaluation_waves(&self) -> Result<Vec<Vec<UnitId>>, Vec<UnitId>> {
        // The consuming unit of each stream
        let mut consumer: Vec<Option<UnitId>> = vec![None; self.streams.len()];
        for (i, unit) in self.units.iter().enumerate() {
            for &s in &unit.inlets {
                consumer[s.as_usize()] = Some(UnitId(i as u16));
            }
        }

        // The unevaluated inlet stream count of each unit
        let mut in_degree: Vec<usize> = self.units.iter().map(|u| u.inlets.len()).collect();

        let mut current: Vec<UnitId> = in_degree
            .iter()
            .enumerate()
            .filter_map(|(i, &d)| (d == 0).then_some(UnitId(i as u16)))
            .collect();

        let mut waves: Vec<Vec<UnitId>> = Vec::new();

        while !current.is_empty() {
            let mut next = Vec::new();
            for &unit in &current {
                for &s in &self.units[unit.as_usize()].outlets {
                    let Some(downstream) = consumer[s.as_usize()] else {
                        continue;
                    };
                    let degree = &mut in_degree[downstream.as_usize()];
                    *degree -= 1;
                    if *degree == 0 {
                        next.push(downstream);
                    }
                }
            }

            waves.push(std::mem::replace(&mut current, next));
        }

        // Any leftover units with unevaluated inlet streams are a part of, or downstream of, a cycle
        let leftover: Vec<UnitId> = in_degree
            .iter()
            .enumerate()
            .filter_map(|(i, &d)| (d > 0).then_some(UnitId(i as u16)))
            .collect();

        if leftover.is_empty() {
            Ok(waves)
        } else {
            Err(leftover)
        }
    }
}
