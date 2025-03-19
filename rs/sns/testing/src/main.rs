use std::process::exit;

use clap::Parser;
use ic_nervous_system_agent::helpers::sns::get_principal_neurons;
use ic_nervous_system_agent::CallCanisters;
use ic_nervous_system_integration_tests::pocket_ic_helpers::load_registry_mutations;
use ic_sns_cli::utils::get_agent;
use ic_sns_testing::nns_dapp::bootstrap_nns;
use ic_sns_testing::sns::{
    complete_sns_swap, create_sns, find_sns_by_name, upgrade_sns_controlled_test_canister,
    TestCanisterInitArgs,
};
use ic_sns_testing::utils::{
    get_identity_principal, get_nns_neuron_hotkeys, validate_network as validate_network_impl,
    validate_target_canister, NNS_NEURON_ID, TREASURY_PRINCIPAL_ID,
};
use ic_sns_testing::{
    BasicScenarioArgs, NnsInitArgs, RunSubCommand, SnsTestingArgs, SnsTestingSubCommand,
    SwapCompleteArgs, ValidateNetworkArgs,
};
use icp_ledger::Tokens;
use pocket_ic::PocketIcBuilder;

async fn nns_init(args: NnsInitArgs) {
    let mut pocket_ic = PocketIcBuilder::new()
        .with_server_url(args.server_url)
        .with_state_dir(args.state_dir.clone())
        .with_nns_subnet()
        .with_sns_subnet()
        .with_ii_subnet()
        .with_application_subnet()
        .build_async()
        .await;
    let endpoint = pocket_ic.make_live(Some(8080)).await;
    println!("PocketIC endpoint: {}", endpoint);

    let registry_proto_path = args.state_dir.join("registry.proto");
    let initial_mutations = load_registry_mutations(registry_proto_path);
    let dev_principal_id = get_identity_principal(&args.dev_identity).unwrap();

    bootstrap_nns(
        &pocket_ic,
        vec![initial_mutations],
        vec![
            (
                (*TREASURY_PRINCIPAL_ID).into(),
                Tokens::from_tokens(10_000_000).unwrap(),
            ),
            (dev_principal_id.into(), Tokens::from_tokens(100).unwrap()),
        ],
        vec![dev_principal_id],
    )
    .await;
    println!("NNS initialized");
    println!(
        "Use the following Neuron ID for further testing: {}",
        NNS_NEURON_ID.id
    );
}

async fn run_basic_scenario(args: BasicScenarioArgs) {
    let dev_agent = &get_agent(&args.network, args.dev_identity).await.unwrap();

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
    )
    .await;
    println!("SNS created");
    println!("Upgrading SNS-controlled test canister...");
    upgrade_sns_controlled_test_canister(
        dev_agent,
        sns,
        args.test_canister_id,
        TestCanisterInitArgs {
            greeting: Some("Hi".to_string()),
        },
    )
    .await;
    println!("Test canister upgraded")
}

async fn validate_network(args: ValidateNetworkArgs) {
    let agent = get_agent(&args.network, None).await.unwrap();

    let network_validation_errors = validate_network_impl(&agent).await;
    if !network_validation_errors.is_empty() {
        eprintln!("SNS-testing failed to validate the target network:");
        for error in &network_validation_errors {
            eprintln!("{}", error);
        }
        exit(1);
    }
}

async fn swap_complete(args: SwapCompleteArgs) {
    let agent = get_agent(&args.network, None).await.unwrap();

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

#[tokio::main]
async fn main() {
    let args = SnsTestingArgs::parse();

    match args.subcommand {
        SnsTestingSubCommand::NnsInit(args) => nns_init(args).await,
        SnsTestingSubCommand::Run { subcommand } => match subcommand {
            RunSubCommand::ValidateNetwork(args) => validate_network(args).await,
            RunSubCommand::BasicScenario(args) => run_basic_scenario(args).await,
            RunSubCommand::SwapComplete(args) => swap_complete(args).await,
        },
    }
}
