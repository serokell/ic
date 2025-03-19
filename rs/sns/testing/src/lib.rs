use std::path::PathBuf;

use clap::{ArgGroup, Parser};
use ic_base_types::{CanisterId, PrincipalId};
use ic_sns_cli::neuron_id_to_candid_subaccount::ParsedSnsNeuron;
use url::Url;

pub mod nns_dapp;
pub mod sns;
pub mod utils;

#[derive(Debug, Parser)]
#[clap(name = "sns-testing-cli", about = "A CLI for testing SNS", version)]
pub struct SnsTestingArgs {
    #[clap(subcommand)]
    pub subcommand: SnsTestingSubCommand,
}

#[derive(Debug, Parser)]
pub enum SnsTestingSubCommand {
    /// Run action on IC network.
    Run {
        #[clap(subcommand)]
        subcommand: RunSubCommand,
    },
    /// Start the new PocketIC-based network with NNS canisters.
    /// exposes the newly created network on 'http://127.0.0.1:8080'
    NnsInit(NnsInitArgs),
}

#[derive(Debug, Parser)]
pub enum RunSubCommand {
    /// Check that the provided IC network has initialized NNS.
    ValidateNetwork(ValidateNetworkArgs),
    /// Run the SNS lifecycle scenario.
    /// The scenario will create the new SNS, and perform an upgrade for the SNS-controlled canister.
    BasicScenario(BasicScenarioArgs),
    /// Complete the SNS swap by providing sufficient direct participations.
    SwapComplete(SwapCompleteArgs),
}

#[derive(Debug, Parser)]
pub struct ValidateNetworkArgs {
    /// The network to validate. This can be either dfx-compatible named network
    /// identifier or the URL of a IC HTTP endpoint.
    #[arg(long)]
    pub network: String,
}

#[derive(Debug, Parser)]
pub struct BasicScenarioArgs {
    /// The network to run the basic scenario on. This can be either dfx-compatible named network
    /// identifier or the URL of a IC HTTP endpoint.
    #[arg(long)]
    pub network: String,
    /// The name of the 'dfx' identity to use for the scenario. This identity is used to submit NNS
    /// proposal to create the new SNS and is added as an initial neuron in the new SNS.
    #[arg(long)]
    pub dev_identity: Option<String>,
    /// The ID of the canister to be controlled by the SNS created in the scenario.
    #[arg(long)]
    pub test_canister_id: CanisterId,
}

#[derive(Debug, Parser)]
#[clap(group(ArgGroup::new("neuron-follow-selection").multiple(false).required(false)))]
pub struct SwapCompleteArgs {
    /// The network to run the basic scenario on. This can be either dfx-compatible named network
    /// identifier or the URL of a IC HTTP endpoint.
    #[arg(long)]
    pub network: String,
    #[arg(long)]
    /// The name of the SNS to complete the swap for.
    pub sns_name: String,
    /// The neuron that swap participants will follow.
    #[clap(long, group = "neuron-follow-selection")]
    pub follow_neuron: Option<ParsedSnsNeuron>,
    /// Principal ID whose neurons swap participants will follow.
    #[clap(long, group = "neuron-follow-selection")]
    pub follow_principal_neurons: Option<PrincipalId>,
}

#[derive(Debug, Parser)]
pub struct NnsInitArgs {
    /// The URL of the 'pocket-ic-server' instance.
    #[arg(long)]
    pub server_url: Url,
    /// The path to the state PocketIC instance state directory.
    #[arg(long)]
    pub state_dir: PathBuf,
    /// The name of the 'dfx' identity. The principal of this identity will be used as the
    /// hotkey for the NNS neuron with the majority voting power.
    #[arg(long)]
    pub dev_identity: String,
}
