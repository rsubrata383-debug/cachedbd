//! Property-based tests for [`cachedb::store::RobinHoodTable`].
//!
//! Includes a custom deterministic XorShift RNG harness and automated test shrinker
//! without external testing dependencies.

use std::collections::HashMap;

use cachedb::store::RobinHoodTable;

/// Deterministic 64-bit XorShift pseudorandom number generator.
#[derive(Debug, Clone)]
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(mut seed: u64) -> Self {
        if seed == 0 {
            seed = 0x853c_49e6_748f_ea9b;
        }
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn gen_range(&mut self, min: usize, max: usize) -> usize {
        if min >= max {
            return min;
        }
        let diff = max - min;
        min + (self.next_u64() as usize % diff)
    }

    fn gen_bool(&mut self) -> bool {
        (self.next_u64() & 1) == 1
    }
}

/// Returns the number of seeds to execute per property test.
fn num_seeds() -> usize {
    std::env::var("CACHEDB_PROP_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(24)
}

/// Shrinks a failing sequence of operations by greedily removing halves, quarters,
/// and individual operations while preserving the property violation.
fn shrink_ops<Op: Clone + std::fmt::Debug>(
    ops: &[Op],
    mut check_fn: impl FnMut(&[Op]) -> Result<(), String>,
) -> (Vec<Op>, String) {
    let mut current = ops.to_vec();
    let Err(mut last_err) = check_fn(&current) else {
        return (current, String::new());
    };

    let mut improved = true;
    while improved && current.len() > 1 {
        improved = false;

        // Try removing fractions: halves (2) and quarters (4)
        for chunk_div in [2, 4] {
            if current.len() < chunk_div {
                continue;
            }
            let chunk_size = current.len() / chunk_div;
            let mut i = 0;
            while i < current.len() {
                let end = (i + chunk_size).min(current.len());
                let mut candidate = current[..i].to_vec();
                candidate.extend_from_slice(&current[end..]);
                if !candidate.is_empty() {
                    if let Err(e) = check_fn(&candidate) {
                        current = candidate;
                        last_err = e;
                        improved = true;
                        break;
                    }
                }
                i += chunk_size;
            }
            if improved {
                break;
            }
        }

        if improved {
            continue;
        }

        // Try removing individual operations one by one
        let mut i = 0;
        while i < current.len() {
            let mut candidate = current[..i].to_vec();
            candidate.extend_from_slice(&current[i + 1..]);
            if !candidate.is_empty() {
                if let Err(e) = check_fn(&candidate) {
                    current = candidate;
                    last_err = e;
                    improved = true;
                    break;
                }
            }
            i += 1;
        }
    }

    (current, last_err)
}

/// Runs a property test across configured seeds, shrinking and reporting failures.
fn run_property<Op: Clone + std::fmt::Debug>(
    test_name: &str,
    mut gen_fn: impl FnMut(&mut XorShift64) -> Vec<Op>,
    mut check_fn: impl FnMut(&[Op]) -> Result<(), String>,
) {
    let seeds = num_seeds();
    for seed_idx in 0..seeds {
        let seed = (seed_idx as u64)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .wrapping_add(1);
        let mut rng = XorShift64::new(seed);
        let ops = gen_fn(&mut rng);

        if let Err(orig_err) = check_fn(&ops) {
            let (shrunk, shrunk_err) = shrink_ops(&ops, &mut check_fn);
            panic!(
                "Property test '{test_name}' FAILED with seed {seed}.\n\
                 Original ops count: {}, initial error: {orig_err}\n\
                 Shrunk to {} ops:\n{:#?}\n\
                 Shrunk failure error: {shrunk_err}",
                ops.len(),
                shrunk.len(),
                shrunk
            );
        }
    }
}

// -----------------------------------------------------------------------------
// Test 1: Mixed Model Test
// -----------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum MixedOp {
    Insert(Vec<u8>, u64),
    Overwrite(Vec<u8>, u64),
    Get(Vec<u8>),
    GetMut(Vec<u8>, u64),
    Remove(Vec<u8>),
    StepMigration(usize),
    Clear,
    Len,
}

#[test]
#[allow(clippy::too_many_lines)]
fn test_mixed_model_property() {
    let key_pool: Vec<Vec<u8>> = (0..24).map(|i| format!("key_{i}").into_bytes()).collect();

    run_property(
        "test_mixed_model_property",
        |rng| {
            let op_count = rng.gen_range(100, 250);
            let mut ops = Vec::with_capacity(op_count);
            for _ in 0..op_count {
                let k = key_pool[rng.gen_range(0, key_pool.len())].clone();
                let op = match rng.gen_range(0, 8) {
                    0 => MixedOp::Insert(k, rng.next_u64()),
                    1 => MixedOp::Overwrite(k, rng.next_u64()),
                    2 => MixedOp::Get(k),
                    3 => MixedOp::GetMut(k, rng.next_u64() % 1000),
                    4 => MixedOp::Remove(k),
                    5 => MixedOp::StepMigration(rng.gen_range(1, 32)),
                    6 => {
                        if rng.gen_range(0, 20) == 0 {
                            MixedOp::Clear
                        } else {
                            MixedOp::Len
                        }
                    }
                    _ => MixedOp::Len,
                };
                ops.push(op);
            }
            ops
        },
        |ops| {
            let mut table = RobinHoodTable::<u64>::with_default_seed(8);
            let mut reference = HashMap::<Vec<u8>, u64>::new();

            for (op_idx, op) in ops.iter().enumerate() {
                match op {
                    MixedOp::Insert(k, v) | MixedOp::Overwrite(k, v) => {
                        let prev_ref = reference.insert(k.clone(), *v);
                        match table.insert(k.clone().into_boxed_slice(), *v) {
                            Ok(prev_tab) => {
                                if prev_tab != prev_ref {
                                    return Err(format!(
                                        "Op {op_idx}: insert({k:?}, {v}) mismatch: table returned {prev_tab:?}, ref returned {prev_ref:?}"
                                    ));
                                }
                            }
                            Err(e) => {
                                return Err(format!("Op {op_idx}: table insert failed: {e:?}"));
                            }
                        }
                    }
                    MixedOp::Get(k) => {
                        let ref_val = reference.get(k);
                        let tab_val = table.get(k);
                        if tab_val != ref_val {
                            return Err(format!(
                                "Op {op_idx}: get({k:?}) mismatch: table={tab_val:?}, ref={ref_val:?}"
                            ));
                        }
                    }
                    MixedOp::GetMut(k, delta) => {
                        let ref_exists = reference.contains_key(k);
                        if ref_exists {
                            if let Some(ref_val) = reference.get_mut(k) {
                                *ref_val = ref_val.wrapping_add(*delta);
                            }
                            match table.get_mut(k) {
                                Some(tab_val) => {
                                    *tab_val = tab_val.wrapping_add(*delta);
                                }
                                None => {
                                    return Err(format!(
                                        "Op {op_idx}: get_mut({k:?}) returned None but key exists in reference"
                                    ));
                                }
                            }
                        } else if table.get_mut(k).is_some() {
                            return Err(format!(
                                "Op {op_idx}: get_mut({k:?}) returned Some but key does not exist in reference"
                            ));
                        }
                    }
                    MixedOp::Remove(k) => {
                        let ref_val = reference.remove(k);
                        let tab_val = table.remove(k);
                        if tab_val != ref_val {
                            return Err(format!(
                                "Op {op_idx}: remove({k:?}) mismatch: table={tab_val:?}, ref={ref_val:?}"
                            ));
                        }
                    }
                    MixedOp::StepMigration(n) => {
                        if let Err(e) = table.step_migration(*n) {
                            return Err(format!("Op {op_idx}: step_migration({n}) error: {e:?}"));
                        }
                    }
                    MixedOp::Clear => {
                        reference.clear();
                        table.clear();
                    }
                    MixedOp::Len => {
                        if table.len() != reference.len() {
                            return Err(format!(
                                "Op {op_idx}: len mismatch: table={}, ref={}",
                                table.len(),
                                reference.len()
                            ));
                        }
                    }
                }

                if op_idx % 25 == 0 {
                    table.check_invariants()?;
                }
            }

            table.check_invariants()?;

            if table.len() != reference.len() {
                return Err(format!(
                    "Final len mismatch: table={}, ref={}",
                    table.len(),
                    reference.len()
                ));
            }

            // Verify iter() yields identical key-value set
            let mut table_items: Vec<(Vec<u8>, u64)> =
                table.iter().map(|(k, v)| (k.to_vec(), *v)).collect();
            table_items.sort();

            let mut ref_items: Vec<(Vec<u8>, u64)> = reference.into_iter().collect();
            ref_items.sort();

            if table_items != ref_items {
                return Err(format!(
                    "iter() mismatch: table has {} items, ref has {} items",
                    table_items.len(),
                    ref_items.len()
                ));
            }

            Ok(())
        },
    );
}

// -----------------------------------------------------------------------------
// Test 2: Key Shapes Test
// -----------------------------------------------------------------------------

#[test]
fn test_key_shapes_property() {
    run_property(
        "test_key_shapes_property",
        |rng| {
            let mut keys = Vec::new();
            // 1. Empty key
            keys.push(vec![]);

            // 2. 1-byte keys
            for b in [0u8, 1, 42, 127, 255] {
                keys.push(vec![b]);
            }

            // 3. Prefix keys
            keys.push(b"prefix".to_vec());
            keys.push(b"prefix_middle".to_vec());
            keys.push(b"prefix_middle_long_suffix".to_vec());

            // 4. Embedded zero bytes
            keys.push(b"user\0id\0session".to_vec());
            keys.push(b"\0\0leading_zero".to_vec());
            keys.push(b"trailing_zero\0\0".to_vec());

            // 5. All-zero keys of various lengths
            for len in [1, 8, 32, 64] {
                keys.push(vec![0u8; len]);
            }

            // 6. Binary keys containing all byte values
            let full_binary: Vec<u8> = (0u8..=255u8).collect();
            keys.push(full_binary);

            // 7. Long keys up to 4 KB
            for size in [512, 1024, 2048, 4096] {
                let mut long_key = vec![0u8; size];
                for (idx, byte) in long_key.iter_mut().enumerate() {
                    *byte = ((idx.wrapping_mul(31)) ^ (size & 0xff)) as u8;
                }
                keys.push(long_key);
            }

            let op_count = rng.gen_range(80, 150);
            let mut ops = Vec::with_capacity(op_count);
            for _ in 0..op_count {
                let k = keys[rng.gen_range(0, keys.len())].clone();
                match rng.gen_range(0, 3) {
                    0 => ops.push((0u8, k, rng.next_u64())), // insert
                    1 => ops.push((1u8, k, 0)),              // get
                    _ => ops.push((2u8, k, 0)),              // remove
                }
            }
            ops
        },
        |ops| {
            let mut table = RobinHoodTable::<u64>::with_default_seed(8);
            let mut reference = HashMap::<Vec<u8>, u64>::new();

            for (op_idx, (tag, key, val)) in ops.iter().enumerate() {
                match tag {
                    0 => {
                        let prev_ref = reference.insert(key.clone(), *val);
                        let prev_tab = table
                            .insert(key.clone().into_boxed_slice(), *val)
                            .map_err(|e| format!("insert err: {e:?}"))?;
                        if prev_tab != prev_ref {
                            return Err(format!(
                                "Op {op_idx}: insert mismatch for key len {}",
                                key.len()
                            ));
                        }
                    }
                    1 => {
                        let tab_val = table.get(key);
                        let ref_val = reference.get(key);
                        if tab_val != ref_val {
                            return Err(format!(
                                "Op {op_idx}: get mismatch for key len {}",
                                key.len()
                            ));
                        }
                    }
                    _ => {
                        let tab_val = table.remove(key);
                        let ref_val = reference.remove(key);
                        if tab_val != ref_val {
                            return Err(format!(
                                "Op {op_idx}: remove mismatch for key len {}",
                                key.len()
                            ));
                        }
                    }
                }
            }

            table.check_invariants()?;
            Ok(())
        },
    );
}

// -----------------------------------------------------------------------------
// Test 3: Heavy Collisions Test
// -----------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum CollisionOp {
    InsertWithHash(u64, Vec<u8>, u64),
    GetWithHash(u64, Vec<u8>),
    RemoveWithHash(u64, Vec<u8>),
    StepMigration(usize),
}

#[test]
fn test_heavy_collisions_property() {
    run_property(
        "test_heavy_collisions_property",
        |rng| {
            let op_count = rng.gen_range(100, 200);
            let mut ops = Vec::with_capacity(op_count);

            for _i in 0..op_count {
                let key_id = rng.gen_range(0, 30);
                let key = format!("coll_key_{key_id}").into_bytes();
                // Ensure a key's forced hash is deterministic for that key
                let forced_hash = if (key_id % 2) == 0 {
                    // hash = i % 8 forces severe collisions in the same bucket
                    (key_id as u64) % 8
                } else {
                    // top of range forces circular wraparound around the end of the array
                    u64::MAX.wrapping_sub((key_id as u64) % 4)
                };

                let op = match rng.gen_range(0, 3) {
                    0 => CollisionOp::InsertWithHash(forced_hash, key, rng.next_u64()),
                    1 => {
                        if rng.gen_bool() {
                            CollisionOp::RemoveWithHash(forced_hash, key)
                        } else {
                            CollisionOp::StepMigration(rng.gen_range(1, 16))
                        }
                    }
                    _ => CollisionOp::GetWithHash(forced_hash, key),
                };
                ops.push(op);
            }
            ops
        },
        |ops| {
            let mut table = RobinHoodTable::<u64>::with_default_seed(8);
            let mut reference = HashMap::<Vec<u8>, u64>::new();

            for (op_idx, op) in ops.iter().enumerate() {
                match op {
                    CollisionOp::InsertWithHash(h, k, v) => {
                        let prev_ref = reference.insert(k.clone(), *v);
                        let prev_tab = table
                            .insert_with_hash(*h, k.clone().into_boxed_slice(), *v)
                            .map_err(|e| format!("err: {e:?}"))?;
                        if prev_tab != prev_ref {
                            return Err(format!("Op {op_idx}: insert_with_hash mismatch"));
                        }
                    }
                    CollisionOp::GetWithHash(h, k) => {
                        let tab_val = table.get_with_hash(*h, k);
                        let ref_val = reference.get(k);
                        if tab_val != ref_val {
                            return Err(format!("Op {op_idx}: get_with_hash mismatch"));
                        }
                    }
                    CollisionOp::RemoveWithHash(h, k) => {
                        let tab_val = table.remove_with_hash(*h, k);
                        let ref_val = reference.remove(k);
                        if tab_val != ref_val {
                            return Err(format!("Op {op_idx}: remove_with_hash mismatch"));
                        }
                    }
                    CollisionOp::StepMigration(n) => {
                        table
                            .step_migration(*n)
                            .map_err(|e| format!("step migration: {e:?}"))?;
                    }
                }

                if op_idx % 20 == 0 {
                    table.check_invariants()?;
                }
            }

            table.check_invariants()?;
            Ok(())
        },
    );
}

// -----------------------------------------------------------------------------
// Test 4: Resize Rule Properties
// -----------------------------------------------------------------------------

#[test]
fn test_resize_rules_property() {
    run_property(
        "test_resize_rules_property",
        |rng| {
            let count = rng.gen_range(50, 120);
            (0..count)
                .map(|i| (format!("key_{i}").into_bytes(), rng.next_u64()))
                .collect::<Vec<_>>()
        },
        |entries| {
            let mut table = RobinHoodTable::<u64>::with_default_seed(8);

            for (k, v) in entries {
                let len_before = table.len();
                let is_new = table.get(k).is_none();
                table
                    .insert(k.clone().into_boxed_slice(), *v)
                    .map_err(|e| format!("insert err: {e:?}"))?;

                if is_new {
                    if table.len() != len_before + 1 {
                        return Err("len() did not increment on inserting new key".to_string());
                    }
                } else if table.len() != len_before {
                    return Err("len() changed on overwriting existing key".to_string());
                }

                if table.is_resizing() {
                    let len_during = table.len();
                    // Migration steps do not change total len
                    table
                        .step_migration(3)
                        .map_err(|e| format!("step_migration err: {e:?}"))?;
                    if table.len() != len_during {
                        return Err(
                            "step_migration changed table.len() during active resize".to_string()
                        );
                    }
                }
                table.check_invariants()?;
            }

            // Property 1: step_migration(usize::MAX) completes migration and old_table becomes None
            if table.is_resizing() {
                let finished = !table
                    .step_migration(usize::MAX)
                    .map_err(|e| format!("err: {e:?}"))?;
                if !finished || table.is_resizing() {
                    return Err("table still resizing after step_migration(usize::MAX)".to_string());
                }
            }

            table.check_invariants()?;
            Ok(())
        },
    );
}

// -----------------------------------------------------------------------------
// Test 5: Order Independence Property
// -----------------------------------------------------------------------------

#[test]
fn test_order_independence_property() {
    run_property(
        "test_order_independence_property",
        |rng| {
            let count = rng.gen_range(30, 80);
            let pairs: Vec<(Vec<u8>, u64)> = (0..count)
                .map(|i| (format!("perm_key_{i}").into_bytes(), rng.next_u64()))
                .collect();

            pairs
        },
        |pairs| {
            let mut table_a = RobinHoodTable::<u64>::with_default_seed(8);
            let mut table_b = RobinHoodTable::<u64>::with_default_seed(8);

            // Permutation A: forward order
            for (k, v) in pairs {
                table_a
                    .insert(k.clone().into_boxed_slice(), *v)
                    .map_err(|e| format!("insert err: {e:?}"))?;
            }

            // Permutation B: reverse order
            for (k, v) in pairs.iter().rev() {
                table_b
                    .insert(k.clone().into_boxed_slice(), *v)
                    .map_err(|e| format!("insert err: {e:?}"))?;
            }

            // Both tables must have identical len
            if table_a.len() != table_b.len() {
                return Err(format!(
                    "len mismatch between orderings: a={}, b={}",
                    table_a.len(),
                    table_b.len()
                ));
            }

            // All keys from table A must be found in table B with identical value
            for (k, v) in table_a.iter() {
                let b_val = table_b.get(k);
                if b_val != Some(v) {
                    return Err(format!(
                        "Key {k:?} in table_a not found or mismatch in table_b"
                    ));
                }
            }

            // iter() yields the same set of items
            let mut items_a: Vec<(Vec<u8>, u64)> =
                table_a.iter().map(|(k, v)| (k.to_vec(), *v)).collect();
            let mut items_b: Vec<(Vec<u8>, u64)> =
                table_b.iter().map(|(k, v)| (k.to_vec(), *v)).collect();
            items_a.sort();
            items_b.sort();

            if items_a != items_b {
                return Err(
                    "iter() yielded different item sets for permutations A and B".to_string(),
                );
            }

            Ok(())
        },
    );
}

// -----------------------------------------------------------------------------
// Test 6: Overwrite and Remove Properties
// -----------------------------------------------------------------------------

#[test]
fn test_overwrite_and_remove_property() {
    run_property(
        "test_overwrite_and_remove_property",
        |rng| {
            let count = rng.gen_range(40, 100);
            (0..count)
                .map(|i| {
                    (
                        format!("rw_key_{i}").into_bytes(),
                        rng.next_u64(),
                        rng.next_u64(),
                    )
                })
                .collect::<Vec<_>>()
        },
        |triplets| {
            let mut table = RobinHoodTable::<u64>::with_default_seed(8);

            for (k, v1, v2) in triplets {
                // Property 1: insert(k, v1) followed by insert(k, v2) returns Some(v1), and get(k) is v2
                let old1 = table
                    .insert(k.clone().into_boxed_slice(), *v1)
                    .map_err(|e| format!("err: {e:?}"))?;
                if old1.is_some() {
                    return Err("first insert returned Some".to_string());
                }

                let old2 = table
                    .insert(k.clone().into_boxed_slice(), *v2)
                    .map_err(|e| format!("err: {e:?}"))?;
                if old2 != Some(*v1) {
                    return Err(format!("overwrite returned {old2:?}, expected Some({v1})"));
                }

                if table.get(k) != Some(v2) {
                    return Err("get after overwrite did not return v2".to_string());
                }

                // Property 2: remove(k) followed by remove(k) returns None
                let rem1 = table.remove(k);
                if rem1 != Some(*v2) {
                    return Err(format!("remove returned {rem1:?}, expected Some({v2})"));
                }

                let rem2 = table.remove(k);
                if rem2.is_some() {
                    return Err("second remove returned Some, expected None".to_string());
                }

                // Property 3: insert(k, v) after remove(k) succeeds
                let re_ins = table
                    .insert(k.clone().into_boxed_slice(), *v1)
                    .map_err(|e| format!("err: {e:?}"))?;
                if re_ins.is_some() {
                    return Err("insert after remove returned Some".to_string());
                }
                if table.get(k) != Some(v1) {
                    return Err("get after re-insert did not return v1".to_string());
                }
            }

            table.check_invariants()?;
            Ok(())
        },
    );
}

// -----------------------------------------------------------------------------
// Test 7: Deliberately Wrong Reference Model Test (Proves Shrinker Operates)
// -----------------------------------------------------------------------------

#[test]
#[should_panic(expected = "Shrunk to")]
fn test_deliberately_wrong_reference_shrinker() {
    #[derive(Debug, Clone)]
    enum BuggyOp {
        Insert(u32, u64),
        Get(u32),
    }

    // Large failing operation sequence
    let mut initial_ops = Vec::new();
    for i in 0..100 {
        initial_ops.push(BuggyOp::Insert(i % 10, u64::from(i)));
        initial_ops.push(BuggyOp::Get(i % 10));
    }

    // Buggy reference model: drops key 7 completely
    let check_buggy = |ops: &[BuggyOp]| -> Result<(), String> {
        let mut table = RobinHoodTable::<u64>::with_default_seed(8);
        for op in ops {
            match op {
                BuggyOp::Insert(k, v) => {
                    let k_bytes = k.to_be_bytes();
                    table
                        .insert(Box::new(k_bytes), *v)
                        .map_err(|e| format!("{e:?}"))?;
                }
                BuggyOp::Get(k) => {
                    let k_bytes = k.to_be_bytes();
                    let tab_val = table.get(&k_bytes);
                    // DELIBERATE BUG in reference model: simulates key 7 being missing
                    let buggy_ref_val = if *k == 7 { None } else { tab_val.copied() };

                    if tab_val.copied() != buggy_ref_val {
                        return Err(format!("Buggy reference mismatch on key {k}"));
                    }
                }
            }
        }
        Ok(())
    };

    let (shrunk, shrunk_err) = shrink_ops(&initial_ops, check_buggy);

    // Verify shrinker reduced the sequence down to a minimal pair (Insert(7, _), Get(7))
    assert!(
        shrunk.len() <= 4,
        "Shrinker did not minimize sequence: got {} ops",
        shrunk.len()
    );

    // Panic with expected message to satisfy #[should_panic]
    panic!(
        "Deliberately wrong reference test panicked successfully!\n\
         Shrunk to {} ops:\n{:#?}\nShrunk error: {shrunk_err}",
        shrunk.len(),
        shrunk
    );
}
