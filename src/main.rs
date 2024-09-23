use rand::{rngs::ThreadRng, Rng, thread_rng};
use std::vec;

#[derive(Debug, Clone, Copy)]
pub struct OutputGroup {
    pub value: u64,
    pub weight: u32,
    pub input_count: usize,
    pub is_segwit: bool,
    pub creation_sequence: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct CoinSelectionOpt {
    pub target_value: u64,
    pub target_feerate: f32,
    pub long_term_feerate: Option<f32>,
    pub min_absolute_fee: u64,
    pub base_weight: u32,
    pub drain_weight: u32,
    pub drain_cost: u64,
    pub cost_per_input: u64,
    pub cost_per_output: u64,
    pub min_drain_value: u64,
    pub excess_strategy: ExcessStrategy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExcessStrategy {
    ToFee,
    ToRecipient,
    ToDrain,
}

#[derive(Debug)]
pub enum SelectionError {
    InsufficientFunds,
    NoSolutionFound,
}

#[derive(Debug)]
pub struct WasteMetric(u64);

#[derive(Debug)]
pub struct SelectionOutput {
    pub selected_inputs: Vec<usize>,
    pub waste: WasteMetric,
}

#[derive(Debug)]
pub struct MatchParameters {
    target_for_match: u64,
    match_range: u64,
    target_feerate: f32,
}

pub fn select_coin_bnb(
    inputs: &[OutputGroup],
    options: CoinSelectionOpt,
) -> Result<SelectionOutput, SelectionError> {
    let mut selected_inputs: Vec<usize> = vec![];
    let mut bnb_tries: u32 = 1_000_000;
    let rng = &mut thread_rng();
    let match_parameters = MatchParameters {
        target_for_match: options.target_value
            + calculate_fee(options.base_weight, options.target_feerate)
            + options.cost_per_output,
        match_range: options.cost_per_input + options.cost_per_output,
        target_feerate: options.target_feerate,
    };
    println!("Match Parameters: {:?}", match_parameters);
    let mut sorted_inputs: Vec<(usize, OutputGroup)> = inputs
        .iter()
        .enumerate()
        .map(|(index, input)| (index, *input))
        .collect();
    sorted_inputs.sort_by_key(|(_, input)| std::cmp::Reverse(input.value));
    println!("Sorted Inputs:");
    for input in &sorted_inputs {
        println!("{:?} ", input);
    }
    let bnb_selected_coin = bnb(
        &sorted_inputs,
        &mut selected_inputs,
        0,
        0,
        &mut bnb_tries,
        rng,
        &match_parameters,
    );
    match bnb_selected_coin {
        Some(selected_coin) => {
            let accumulated_value: u64 = selected_coin
                .iter()
                .fold(0, |acc, &i| acc + inputs[i].value);
            let accumulated_weight: u32 = selected_coin
                .iter()
                .fold(0, |acc, &i| acc + inputs[i].weight);
            let estimated_fee = calculate_fee(accumulated_weight, options.target_feerate);
            let waste = calculate_waste(
                inputs,
                &selected_inputs,
                &options,
                accumulated_value,
                accumulated_weight,
                estimated_fee,
            );
            let selection_output = SelectionOutput {
                selected_inputs: selected_coin.clone(),
                waste: WasteMetric(waste),
            };
            println!("Selected UTXOs: {:?}", selected_coin);
            println!("Accumulated Value: {}", accumulated_value);
            println!("Accumulated Weight: {}", accumulated_weight);
            println!("Estimated Fee: {}", estimated_fee);
            println!("Waste: {}", waste);
            println!("Selection Output: {:?}", selection_output);
            Ok(selection_output)
        }
        None => {
            println!("No solution found");
            Err(SelectionError::NoSolutionFound)
        }
    }
}

fn bnb(
    inputs_in_desc_value: &[(usize, OutputGroup)],
    selected_inputs: &mut Vec<usize>,
    acc_eff_value: u64,
    depth: usize,
    bnb_tries: &mut u32,
    rng: &mut ThreadRng,
    match_parameters: &MatchParameters,
) -> Option<Vec<usize>> {
    println!(
        "bnb called with acc_eff_value: {}, depth: {}, bnb_tries: {}, target_for_match: {}, match_range: {}",
        acc_eff_value, depth, bnb_tries, match_parameters.target_for_match, match_parameters.match_range
    );
    if acc_eff_value > match_parameters.target_for_match + match_parameters.match_range {
        println!("Exceeded match range");
        return None;
    }
    if acc_eff_value >= match_parameters.target_for_match {
        println!("Match found with selected inputs: {:?}", selected_inputs);
        return Some(selected_inputs.to_vec());
    }
    *bnb_tries -= 1;
    if *bnb_tries == 0 || depth >= inputs_in_desc_value.len() {
        println!("No more tries or depth exceeded");
        return None;
    }
    if rng.gen_bool(0.5) {
        let new_effective_value = acc_eff_value
            + effective_value(
                &inputs_in_desc_value[depth].1,
                match_parameters.target_feerate,
            );
        selected_inputs.push(inputs_in_desc_value[depth].0);
        println!("Selected UTXO: {:?}", inputs_in_desc_value[depth]);
        let with_this = bnb(
            inputs_in_desc_value,
            selected_inputs,
            new_effective_value,
            depth + 1,
            bnb_tries,
            rng,
            match_parameters,
        );
        match with_this {
            Some(_) => with_this,
            None => {
                selected_inputs.pop();
                println!("Popped UTXO: {:?}", inputs_in_desc_value[depth]);
                bnb(
                    inputs_in_desc_value,
                    selected_inputs,
                    acc_eff_value,
                    depth + 1,
                    bnb_tries,
                    rng,
                    match_parameters,
                )
            }
        }
    } else {
        match bnb(
            inputs_in_desc_value,
            selected_inputs,
            acc_eff_value,
            depth + 1,
            bnb_tries,
            rng,
            match_parameters,
        ) {
            Some(without_this) => Some(without_this),
            None => {
                let new_effective_value = acc_eff_value
                    + effective_value(
                        &inputs_in_desc_value[depth].1,
                        match_parameters.target_feerate,
                    );
                selected_inputs.push(inputs_in_desc_value[depth].0);
                println!("Selected UTXO: {:?}", inputs_in_desc_value[depth]);
                let with_this = bnb(
                    inputs_in_desc_value,
                    selected_inputs,
                    new_effective_value,
                    depth + 1,
                    bnb_tries,
                    rng,
                    match_parameters,
                );
                match with_this {
                    Some(_) => with_this,
                    None => {
                        selected_inputs.pop();
                        println!("Popped UTXO: {:?}", inputs_in_desc_value[depth]);
                        None
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
    let mut waste: u64 = 0;
    if let Some(long_term_feerate) = options.long_term_feerate {
        waste += (estimated_fee as f32
            - selected_inputs.len() as f32 * long_term_feerate * accumulated_weight as f32)
            .ceil() as u64;
    }
    if options.excess_strategy != ExcessStrategy::ToDrain {
        waste += accumulated_value - options.target_value - estimated_fee;
    } else {
        waste += options.drain_cost;
    }
    println!("Calculated Waste: {}", waste);
    waste
}

#[inline]
fn calculate_fee(weight: u32, rate: f32) -> u64 {
    let fee = (weight as f32 * rate).ceil() as u64;
    println!("Calculated Fee: {}", fee);
    fee
}

#[inline]
fn effective_value(output: &OutputGroup, feerate: f32) -> u64 {
    let eff_value = output
        .value
        .saturating_sub(calculate_fee(output.weight, feerate));
    println!("Effective Value for {:?}: {}", output, eff_value);
    eff_value
}

#[cfg(test)]
mod test {
    use super::*;

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

    fn new_setup_options(target_value: u64, target_feerate: f32, long_term_feerate: Option<f32>) -> CoinSelectionOpt {
        CoinSelectionOpt {
            target_value,
            target_feerate,
            long_term_feerate,
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

    #[test]
    fn test_bnb_solution() {
        let values = [
            OutputGroup {
                value: 55000,
                weight: 500,
                input_count: 1,
                is_segwit: false,
                creation_sequence: None,
            },
            OutputGroup {
                value: 400,
                weight: 200,
                input_count: 1,
                is_segwit: false,
                creation_sequence: None,
            },
            OutputGroup {
                value: 40000,
                weight: 300,
                input_count: 1,
                is_segwit: false,
                creation_sequence: None,
            },
            OutputGroup {
                value: 25000,
                weight: 100,
                input_count: 1,
                is_segwit: false,
                creation_sequence: None,
            },
            OutputGroup {
                value: 35000,
                weight: 150,
                input_count: 1,
                is_segwit: false,
                creation_sequence: None,
            },
            OutputGroup {
                value: 600,
                weight: 250,
                input_count: 1,
                is_segwit: false,
                creation_sequence: None,
            },
            OutputGroup {
                value: 30000,
                weight: 120,
                input_count: 1,
                is_segwit: false,
                creation_sequence: None,
            },
            OutputGroup {
                value: 5000,
                weight: 50,
                input_count: 1,
                is_segwit: false,
                creation_sequence: None,
            },
        ];
        let opt = new_setup_options(5730, 0.01, None);
        let ans = select_coin_bnb(&values, opt);
        if let Ok(selection_output) = ans {
            let expected_solution = vec![7, 5, 1];
            assert_eq!(
                selection_output.selected_inputs, expected_solution,
                "Expected solution {:?}, but got {:?}",
                expected_solution, selection_output.selected_inputs
            );
        } else {
            assert!(false, "Failed to find a solution");
        }
    }

    #[test]
    fn test_bnb_no_solution() {
        let inputs = setup_basic_output_groups();
        let total_input_value: u64 = inputs.iter().map(|input| input.value).sum();
        let impossible_target = total_input_value + 1000;
        let options = new_setup_options(impossible_target, 0.5, Some(0.1));
        let result = select_coin_bnb(&inputs, options);
        assert!(
            matches!(result, Err(SelectionError::NoSolutionFound)),
            "Expected NoSolutionFound error, got {:?}",
            result
        );
    }
}

fn main() {
    println!("Coinselector");
}