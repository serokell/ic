use clap::Parser;
use ic_nervous_system_integration_tests::pocket_ic_helpers::load_registry_mutations;
use ic_nns_common::pb::v1::NeuronId;
use ic_sns_testing::nns_dapp::bootstrap_nns;
use ic_sns_testing::utils::{
    get_identity_principal, DEFAULT_POWERFUL_NNS_NEURON_ID, TREASURY_PRINCIPAL_ID,
};
use ic_sns_testing::NnsInitArgs;
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

    let treasury_principal_id = if let Some(icp_treasury_identity) = args.icp_treasury_identity {
        get_identity_principal(&icp_treasury_identity).unwrap()
    } else {
        *TREASURY_PRINCIPAL_ID
    };

    let deciding_nns_neuron_id = args
        .deciding_nns_neuron_id
        .map(|id| NeuronId { id })
        .unwrap_or(DEFAULT_POWERFUL_NNS_NEURON_ID);

    bootstrap_nns(
        &pocket_ic,
        vec![initial_mutations],
        vec![
            (
                treasury_principal_id.into(),
                Tokens::from_tokens(10_000_000).unwrap(),
            ),
            (dev_principal_id.into(), Tokens::from_tokens(100).unwrap()),
        ],
        dev_principal_id,
        deciding_nns_neuron_id,
    )
    .await;
    println!("NNS initialized");
    println!(
        "Use the following Neuron ID for further testing: {}",
        deciding_nns_neuron_id.id
    );
}

#[tokio::main]
async fn main() {
    let args = NnsInitArgs::parse();
    nns_init(args).await
}
