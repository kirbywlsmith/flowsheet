fn main() {
    println!("Hello, world!");
}

// ============================================================================
// TARGET FLOWSHEET — "simple recycle circuit"
//
// Topology
// --------
//   Feed                     (source, 0 in / 1 out)
//     │ S0
//     ▼
//   Mixer  ◄─────────────┐   (2 in / 1 out)
//     │ S1               │
//     ▼                  │ S4   recycle — this is the TEAR STREAM
//   Conditioning tank    │
//     │ S2               │   (1 in / 1 out)
//     ▼                  │
//   Splitter ────────────┘   (1 in / 2 out)
//     │ S3
//     ▼
//   Product                  (sink, 1 in / 0 out)
//
// Streams: S0..S4. Units: U0..U4 in the order listed above.
// The Feed→Mixer→Tank→Splitter→Mixer cycle is why a topological sort alone
// can't solve this — S4 must be guessed and iterated to convergence.
//
// Species (fixed list, index = SpeciesId)
// ---------------------------------------
//   0  CuFeS2(s)   chalcopyrite  — the valuable mineral
//   1  SiO2(s)     gangue        — the worthless rock
//   2  H2O(l)      water
//
// All flows are mass flow rates in t/h. Temperature and pressure are carried
// on the stream but unused until the energy balance milestone.
//
// Unit models
// -----------
//   U0 Feed        outlet = fixed vector [40.0, 360.0, 600.0]
//   U1 Mixer       outlet[i] = sum of inlet[i] over all inlets
//   U2 Tank        outlet = inlet          (pass-through at steady state)
//   U3 Splitter    param: recycle_fraction f = 0.3
//                  recycle_outlet[i] = inlet[i] * f
//                  product_outlet[i] = inlet[i] * (1.0 - f)
//   U4 Product     accumulates inlet, no outlets
//
// Every unit must satisfy: sum(inlets) == sum(outlets), per species.
//
// Expected converged solution (t/h)
// ---------------------------------
//                    CuFeS2     SiO2      H2O       total
//   S0 feed           40.000   360.000   600.000   1000.000
//   S1 mixer out      57.143   514.286   857.143   1428.571
//   S2 tank out       57.143   514.286   857.143   1428.571
//   S3 product        40.000   360.000   600.000   1000.000
//   S4 recycle        17.143   154.286   257.143    428.571
//
// Two invariants worth asserting in tests:
//   1. S3 == S0 exactly. Nothing accumulates at steady state, so whatever
//      enters the plant must leave it — the recycle only inflates the
//      INTERNAL flows, never the product.
//   2. Internal flow amplification is 1/(1-f) = 1/0.7 = 1.42857.
//
// Convergence behaviour
// ---------------------
// Guess S4 = zeros, then direct substitution: the error shrinks by exactly
// f each pass (geometric, ratio 0.3), so ~14 iterations to reach 1e-6.
// Bump f to 0.9 and it takes ~130 — that's the motivation for Wegstein.
//
// Build order
// -----------
//   [ ] SpeciesId + species list, Stream with a dense Vec<f64> of flows
//   [ ] Mixer and Splitter as plain functions, unit-tested in isolation
//   [ ] Arena: Vec<UnitOp>, Vec<Stream>, UnitId/StreamId newtypes
//   [ ] Wire up the graph above by hand in a build_flowsheet() fn
//   [ ] Solve WITHOUT the recycle first (delete S4, Mixer takes only S0)
//       — this is acyclic, so a topological sort solves it in one pass
//   [ ] Add S4 back, detect the cycle, tear at S4, direct substitution
//   [ ] Wegstein acceleration, tolerance + max-iteration config
//
// Later: replace U2 with a flotation cell (1 in / 2 out, per-species
// recovery — say 0.85 for CuFeS2, 0.05 for SiO2, 0.30 for H2O) and route
// its tails to the splitter. The loop then genuinely upgrades the ore and
// the arithmetic stops being checkable by hand.
// ============================================================================
