use rand::{rngs::ThreadRng, thread_rng, Rng};
// use std::cmp::Reverse;
// use std::collections::HashSet;
// use std::hash::{Hash, Hasher};
// use std::sync::{Arc, Mutex};
// use std::thread;
use std::{vec};

/// A [`OutputGroup`] represents an input candidate for Coinselection. This can either be a
/// single UTXO, or a group of UTXOs that should be spent together.
/// The library user is responsible for crafting this structure correctly. Incorrect representation of this
/// structure will cause incorrect selection result.
#[derive(Debug, Clone, Copy)]
pub struct OutputGroup {
    /// Total value of the UTXO(s) that this [`WeightedValue`] represents.
    pub value: u64,
    /// Total weight of including this/these UTXO(s).
    /// `txin` fields: `prevout`, `nSequence`, `scriptSigLen`, `scriptSig`, `scriptWitnessLen`,
    /// `scriptWitness` should all be included.
    pub weight: u32,
    /// The total number of inputs; so we can calculate extra `varint` weight due to `vin` length changes.
    pub input_count: usize,
    /// Whether this [`OutputGroup`] contains at least one segwit spend.
    pub is_segwit: bool,
    /// Relative Creation sequence for this group. Only used for FIFO selection. Specify None, if FIFO
    /// selection is not required.
    /// sequence numbers are arbitrary index only to denote relative age of utxo group among a set of groups.
    /// To denote the oldest utxo group, give them a sequence number of Some(0).
    pub creation_sequence: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct CoinSelectionOpt {
    /// The value we need to select.
    pub target_value: u64,

    /// The feerate we should try and achieve in sats per weight unit.
    pub target_feerate: f32,
    /// The feerate
    pub long_term_feerate: Option<f32>, // TODO: Maybe out of scope? (waste)
    /// The minimum absolute fee. I.e., needed for RBF.
    pub min_absolute_fee: u64,

    /// The weight of the template transaction, including fixed fields and outputs.
    pub base_weight: u32,
    /// Additional weight if we include the drain (change) output.
    pub drain_weight: u32,

    /// Weight of spending the drain (change) output in the future.
    pub drain_cost: u64,

    /// Estimate of cost of spending an input
    pub cost_per_input: u64,

    /// Estimate of cost of spending the output
    pub cost_per_output: u64,

    /// Minimum value allowed for a drain (change) output.
    pub min_drain_value: u64,

    /// Strategy to use the excess value other than fee and target
    pub excess_strategy: ExcessStrategy,
}

/// Strategy to decide what to do with the excess amount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExcessStrategy {
    ToFee,
    ToRecipient,
    ToDrain,
}

/// Error Describing failure of a selection attempt, on any subset of inputs
#[derive(Debug, PartialEq)]
pub enum SelectionError {
    InsufficientFunds,
    NoSolutionFound,
}

/// Wastemetric, of a selection of inputs, is measured in satoshis. It helps evaluate the selection made by different algorithms in the context of the current and long term fee rate.
/// It is used to strike a balance between wanting to minimize the current transaction's fees versus minimizing the overall fees paid by the wallet during its lifetime.
/// During high fee rate environment, selecting fewer number of inputs will help minimize the transaction fees.
/// During low fee rate environment, slecting more number of inputs will help minimize the over all fees paid by the wallet during its lifetime.
/// This is used to compare various selection algorithm and find the most
/// optimizewd solution, represented by least [WasteMetric] value.
#[derive(Debug)]
pub struct WasteMetric(u64);

/// The result of selection algorithm
#[derive(Debug)]
pub struct SelectionOutput {
    /// The selected input indices, refers to the indices of the inputs Slice Reference
    pub selected_inputs: Vec<usize>,
    /// The waste amount, for the above inputs
    pub waste: WasteMetric,
}

/// Perform Coinselection via Branch And Bound algorithm.
pub fn select_coin_bnb(
    inputs: &[OutputGroup],
    options: CoinSelectionOpt,
    rng: &mut ThreadRng,
) -> Result<SelectionOutput, SelectionError> {
    let mut selected_inputs: Vec<usize> = vec![];
    const BNB_TRIES: u32 = 1000000;

    let mut sorted_inputs: Vec<(usize, OutputGroup)> = inputs
        .iter()
        .enumerate()
        .map(|(index, input)| (index, *input))
        .collect();
    sorted_inputs.sort_by_key(|(_, input)| std::cmp::Reverse(input.value));

    let bnb_selected_coin = bnb(
        &sorted_inputs,
        &mut selected_inputs,
        0,
        0,
        BNB_TRIES,
        &options,
        rng,
    );
    match bnb_selected_coin {
        Some(selected_coin) => {
            let accumulated_value: u64 = selected_coin
                .iter()
                .fold(0, |acc, &i| acc + inputs[i].value);
            let accumulated_weight: u32 = selected_coin
                .iter()
                .fold(0, |acc, &i| acc + inputs[i].weight);
            let estimated_fee = 0;
            let waste = calculate_waste(
                inputs,
                &selected_inputs,
                &options,
                accumulated_value,
                accumulated_weight,
                estimated_fee,
            );
            let selection_output = SelectionOutput {
                selected_inputs: selected_coin,
                waste: WasteMetric(waste),
            };
            Ok(selection_output)
        }
        None => Err(SelectionError::NoSolutionFound),
    }
}

/// Return empty vec if no solutions are found
// changing the selected_inputs : &[usize] -> &mut Vec<usize>
fn bnb(
    inputs_in_desc_value: &[(usize, OutputGroup)],
    selected_inputs: &mut Vec<usize>,
    acc_eff_value: u64,
    depth: usize,
    bnp_tries: u32,
    options: &CoinSelectionOpt,
    rng: &mut ThreadRng,
) -> Option<Vec<usize>> {
    let target_for_match = options.target_value
        + calculate_fee(options.base_weight, options.target_feerate)
        + options.cost_per_output;
    let match_range = options.cost_per_input + options.cost_per_output;
    if acc_eff_value > target_for_match + match_range {
        return None;
    }
    if acc_eff_value >= target_for_match {
        return Some(selected_inputs.to_vec());
    }
    if bnp_tries == 0 || depth >= inputs_in_desc_value.len() {
        return None;
    }
    if rng.gen_bool(0.5) {
        // exploring the inclusion branch
        // first include then omit
        let new_effective_values =
            acc_eff_value + effective_value(&inputs_in_desc_value[depth].1, options.target_feerate);
        selected_inputs.push(inputs_in_desc_value[depth].0);
        let with_this = bnb(
            inputs_in_desc_value,
            selected_inputs,
            new_effective_values,
            depth + 1,
            bnp_tries - 1,
            options,
            rng,
        );
        match with_this {
            Some(_) => with_this,
            None => {
                selected_inputs.pop(); //poping out the selected utxo if it does not fit
                let without_this = bnb(
                    inputs_in_desc_value,
                    selected_inputs,
                    acc_eff_value,
                    depth + 1,
                    bnp_tries - 2,
                    options,
                    rng,
                );
                match without_this {
                    Some(_) => without_this,
                    None => None, // this may or may not be correct
                }
            }
        }
    } else {
        let without_this = bnb(
            inputs_in_desc_value,
            selected_inputs,
            acc_eff_value,
            depth + 1,
            bnp_tries - 1,
            options,
            rng,
        );
        match without_this {
            Some(_) => without_this,
            None => {
                let new_effective_values = acc_eff_value
                    + effective_value(&inputs_in_desc_value[depth].1, options.target_feerate);
                selected_inputs.push(inputs_in_desc_value[depth].0);
                let with_this = bnb(
                    inputs_in_desc_value,
                    selected_inputs,
                    new_effective_values,
                    depth + 1,
                    bnp_tries - 2,
                    options,
                    rng,
                );
                match with_this {
                    Some(_) => with_this,
                    None => {
                        selected_inputs.pop(); // poping out the selected utxo if it does not fit
                        None // this may or may not be correct
                    }
                }
            }
        }
    }
}


#[inline]
fn calculate_waste(
    inputs: &[OutputGroup],
    selected_inputs: &[usize],
    options: &CoinSelectionOpt,
    accumulated_value: u64,
    accumulated_weight: u32,
    estimated_fee: u64,
) -> u64 {
    // waste =  weight*(target feerate - long term fee rate) + cost of change + excess
    // weight - total weight of selected inputs
    // cost of change - includes the fees paid on this transaction's change output plus the fees that will need to be paid to spend it later. If there is no change output, the cost is 0.
    // excess - refers to the difference between the sum of selected inputs and the amount we need to pay (the sum of output values and fees). There shouldn’t be any excess if there is a change output.

    let mut waste: u64 = 0;
    if let Some(long_term_feerate) = options.long_term_feerate {
        waste = (accumulated_weight as f32 * (options.target_feerate - long_term_feerate)).ceil()
            as u64;
    }
    if options.excess_strategy != ExcessStrategy::ToDrain {
        // Change is not created if excess strategy is ToFee or ToRecipient. Hence cost of change is added
        waste += accumulated_value - (options.target_value + estimated_fee);
    } else {
        // Change is created if excess strategy is set to ToDrain. Hence 'excess' should be set to 0
        waste += options.drain_cost;
    }
    waste
}

#[inline]
fn calculate_fee(weight: u32, rate: f32) -> u64 {
    (weight as f32 * rate).ceil() as u64
}

/// Returns the effective value which is the actual value minus the estimated fee of the OutputGroup
#[inline]
fn effective_value(output: &OutputGroup, feerate: f32) -> u64 {
    output
        .value
        .saturating_sub(calculate_fee(output.weight, feerate))
}


fn setup_options(target_value: u64) -> CoinSelectionOpt {
    CoinSelectionOpt {
        target_value,
        target_feerate: 0.33, // Simplified feerate
        long_term_feerate: Some(0.4),
        min_absolute_fee: 0,
        base_weight: 10,
        drain_weight: 50,
        drain_cost: 10,
        cost_per_input: 20,
        cost_per_output: 10,
        min_drain_value: 500,
        excess_strategy: ExcessStrategy::ToDrain,
    }
}

fn create_output_group(
    value: u64,
    weight: u32,
    input_count: usize,
    is_segwit: bool,
    creation_sequence: Option<u32>,
) -> OutputGroup {
    OutputGroup {
        value,
        weight,
        input_count,
        is_segwit,
        creation_sequence,
    }
}  

// fn create_output_group() -> Vec<OutputGroup> {
//     vec![
//         OutputGroup {
//             value: 1000,
//             weight: 100,
//             input_count: 1,
//             is_segwit: false,
//             creation_sequence: None,
//         },
//         OutputGroup {
//             value: 2000,
//             weight: 200,
//             input_count: 1,
//             is_segwit: false,
//             creation_sequence: None,
//         },
//         OutputGroup {
//             value: 3000,
//             weight: 300,
//             input_count: 1,
//             is_segwit: false,
//             creation_sequence: None,
//         },
//     ]
// }

#[test]
fn test_bnb_exact_match() {
    let inputs = vec![
        create_output_group(1000, 100, 1, false, None),
        create_output_group(2000, 200, 1, false, None),
        create_output_group(2000, 200, 1, false, None),
    ];
    let options = setup_options(5000);
    let mut rng = thread_rng();

    let result = select_coin_bnb(&inputs, options, &mut rng);
    assert!(result.is_ok(), "Expected Ok(_) value, got Err");
    let selection_output = result.unwrap();
    assert!(!selection_output.selected_inputs.is_empty());

    let selected_values: u64 = selection_output
        .selected_inputs
        .iter()
        .map(|&i| inputs[i].value)
        .sum();
    assert_eq!(selected_values, 5000);
}

#[test]
fn bnb_test_no_match() {
    let inputs = vec![
        create_output_group(1000, 100, 1, false, None),
        create_output_group(1500, 150, 1, false, None),
        create_output_group(2000, 200, 1, false, None),
    ];
    let options = setup_options(5000);
    let mut rng = thread_rng();

    let result = select_coin_bnb(&inputs, options, &mut rng);
    assert!(result.is_err());
}

// #[test]
// fn bnb_test_over_match() {
//     let target_value = 5000;
//     let inputs = vec![
//         create_output_group(3000, 300, 1, false, None),
//         create_output_group(3000, 300, 1, false, None),
//         create_output_group(3000, 300, 1, false, None),
//     ];
//     let options = setup_options(target_value);
//     let mut rng = thread_rng();

//     let result = select_coin_bnb(&inputs, options, &mut rng);
//     assert!(result.is_ok(), "Expected Ok(_) value, got Err");
//     let selection_output = result.unwrap();
//     assert!(!selection_output.selected_inputs.is_empty());
//     assert!(selection_output.selected_inputs.len() >= 2);
// }

#[test]
fn bnb_test_multiple_solutions() {
    let inputs = vec![
        create_output_group(2000, 200, 1, false, None),
        create_output_group(3000, 300, 1, false, None),
        create_output_group(3000, 300, 1, false, None),
    ];
    let options = setup_options(5000);
    let mut rng = thread_rng();

    let result = select_coin_bnb(&inputs, options, &mut rng);
    assert!(result.is_ok(), "Expected Ok(_) value, got Err");
    let selection_output = result.unwrap();
    assert!(!selection_output.selected_inputs.is_empty());
    assert_eq!(selection_output.selected_inputs.len(), 2);
}

#[test]
fn bnb_test_single_input_match() {
    let inputs = vec![create_output_group(5000, 500, 1, false, None)];
    let options = setup_options(5000);
    let mut rng = thread_rng();

    let result = select_coin_bnb(&inputs, options, &mut rng);
    assert!(result.is_ok(), "Expected Ok(_) value, got Err");
    let selection_output = result.unwrap();
    assert!(!selection_output.selected_inputs.is_empty());
    assert_eq!(selection_output.selected_inputs.len(), 1);
    assert_eq!(selection_output.selected_inputs, vec![0]);
}

#[test]
fn bnb_test_random_branching() {
    let inputs = vec![
        create_output_group(1000, 100, 1, false, None),
        create_output_group(2000, 200, 1, false, None),
        create_output_group(3000, 300, 1, false, None),
        create_output_group(4000, 400, 1, false, None),
        create_output_group(5000, 500, 1, false, None),
    ];
    let options = setup_options(5000);
    let mut rng = thread_rng();

    let mut found_solutions = 0;
    for _ in 0..10 {
        let result = select_coin_bnb(&inputs, options, &mut rng);
        if result.is_ok() {
            found_solutions += 1;
        }
    }
    assert!(
        found_solutions > 0,
        "Expected at least one solution, but found none"
    );
}

#[test]
fn bnb_insufficient_bal() {
    let inputs = vec![create_output_group(1000, 100, 1, false, None)];
    let options = setup_options(7000); // Set a target value higher than the sum of all inputs
    let result = select_coin_bnb(&inputs, options, &mut thread_rng());
    assert!(matches!(result, Err(SelectionError::NoSolutionFound)));
}

// Assuming the existence of `create_output_group`, `setup_options`, and other necessary definitions
fn setup_basic_output_groups() -> Vec<OutputGroup> {
    vec![
        OutputGroup {
            value: 1000,
            weight: 100,
            input_count: 1,
            is_segwit: false,
            creation_sequence: None,
        },
        OutputGroup {
            value: 2000,
            weight: 200,
            input_count: 1,
            is_segwit: false,
            creation_sequence: None,
        },
        OutputGroup {
            value: 3000,
            weight: 300,
            input_count: 1,
            is_segwit: false,
            creation_sequence: None,
        },
    ]
}

fn main() {
    let inputs = setup_basic_output_groups();
    let options = setup_options(2000);
    let mut rng = thread_rng();

    match select_coin_bnb(&inputs, options, &mut rng) {
        Ok(selection_output) => println!("Selection output: {:?}", selection_output),
        Err(e) => println!("Error: {:?}", e),
    }
}