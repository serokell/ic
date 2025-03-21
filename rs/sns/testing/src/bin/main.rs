use std::process::exit;

use clap::Parser;
use ic_nervous_system_agent::helpers::sns::get_principal_neurons;
use ic_nervous_system_agent::CallCanisters;
use ic_sns_cli::utils::get_agent;
use ic_sns_testing::sns::{
    complete_sns_swap, create_sns, find_sns_by_name, propose_sns_controlled_test_canister_upgrade,
    sns_proposal_upvote as sns_proposal_upvote_impl, wait_for_sns_controlled_canister_upgrade,
    TestCanisterInitArgs,
};
use ic_sns_testing::utils::{
    get_nns_neuron_hotkeys, validate_network as validate_network_impl, validate_target_canister,
    NNS_NEURON_ID,
};
use ic_sns_testing::{
    RunBasicScenarioArgs, SnsProposalUpvoteArgs, SnsTestingArgs, SnsTestingSubCommand,
    SwapCompleteArgs,
};

async fn run_basic_scenario(network: String, args: RunBasicScenarioArgs) {
    let dev_agent = &get_agent(&network, args.dev_identity).await.unwrap();

    let target_canister_validation_errors =
        validate_target_canister(dev_agent, args.test_canister_id).await;

    if !target_canister_validation_errors.is_empty() {
        eprintln!("SNS-testing failed to validate the test canister:");
        for error in &target_canister_validation_errors {
            eprintln!("{}", error);
        }
        exit(1);
    }

    match get_nns_neuron_hotkeys(dev_agent, NNS_NEURON_ID).await {
        Ok(nns_neuron_hotkeys) => {
            assert!(
                nns_neuron_hotkeys.contains(&dev_agent.caller().unwrap().into()),
                "Developer identity is not a hotkey for NNS neuron"
            );
        }
        Err(err) => {
            panic!(
                "Failed to get NNS neuron {} hotkeys: {}",
                NNS_NEURON_ID.id, err
            );
        }
    }

    println!("Creating SNS...");
    let sns = create_sns(
        dev_agent,
        NNS_NEURON_ID,
        dev_agent,
        vec![args.test_canister_id],
        true,
    )
    .await;
    println!("SNS created");
    println!("Upgrading SNS-controlled test canister...");
    let proposal_id = propose_sns_controlled_test_canister_upgrade(
        dev_agent,
        sns.clone(),
        args.test_canister_id,
        TestCanisterInitArgs {
            greeting: Some("Hi".to_string()),
        },
    )
    .await;
    wait_for_sns_controlled_canister_upgrade(dev_agent, proposal_id, args.test_canister_id, sns)
        .await;
    println!("Test canister upgraded")
}

async fn validate_network(network: String) {
    let agent = get_agent(&network, None).await.unwrap();

    let network_validation_errors = validate_network_impl(&agent).await;
    if !network_validation_errors.is_empty() {
        eprintln!("SNS-testing failed to validate the target network:");
        for error in &network_validation_errors {
            eprintln!("{}", error);
        }
        exit(1);
    }
}

async fn swap_complete(network: String, args: SwapCompleteArgs) {
    let agent = get_agent(&network, None).await.unwrap();

    // Normally, SNSes would have different names, so the vector below would have a single element.
    let target_snses = find_sns_by_name(&agent, args.sns_name.clone()).await;

    if target_snses.is_empty() {
        eprintln!("No SNS found with the name: {}", args.sns_name);
        exit(1);
    }

    for sns in target_snses {
        let mut neurons_to_follow = vec![];
        if let Some(neuron) = &args.follow_neuron {
            neurons_to_follow.push(neuron.0.clone());
        }
        if let Some(principal) = args.follow_principal_neurons {
            let principal_neurons =
                match get_principal_neurons(&agent, sns.governance, principal).await {
                    Ok(neurons) => neurons,
                    Err(e) => {
                        eprintln!("Failed to get principal neurons: {}", e);
                        vec![]
                    }
                };
            neurons_to_follow.extend(principal_neurons);
        }
        if let Err(e) = complete_sns_swap(&agent, sns.swap, sns.governance, neurons_to_follow).await
        {
            eprintln!("Failed to complete swap for SNS {}: {}", args.sns_name, e);
            exit(1);
        }
    }
}

async fn sns_proposal_upvote(network: String, args: SnsProposalUpvoteArgs) {
    let agent = get_agent(&network, None).await.unwrap();

    let target_snses = find_sns_by_name(&agent, args.sns_name.clone()).await;
    let target_sns = target_snses.first();

    if let Some(sns) = target_sns {
        println!(
            "Upvoting proposal {} for SNS \"{}\"...",
            args.proposal_id, args.sns_name
        );
        if let Err(e) = sns_proposal_upvote_impl(
            &agent,
            sns.governance,
            sns.swap,
            args.proposal_id,
            args.wait,
        )
        .await
        {
            eprintln!(
                "Failed to upvote proposal {} for SNS {}: {}",
                args.proposal_id, args.sns_name, e
            );
            exit(1);
        }
    } else {
        eprintln!("No SNS found with the name: {}", args.sns_name);
        exit(1);
    }
}

#[tokio::main]
async fn main() {
    let SnsTestingArgs {
        network,
        subcommand,
    } = SnsTestingArgs::parse();

    match subcommand {
        SnsTestingSubCommand::ValidateNetwork(_) => validate_network(network).await,
        SnsTestingSubCommand::RunBasicScenario(args) => run_basic_scenario(network, args).await,
        SnsTestingSubCommand::SwapComplete(args) => swap_complete(network, args).await,
        SnsTestingSubCommand::SnsProposalUpvote(args) => sns_proposal_upvote(network, args).await,
    }
}
