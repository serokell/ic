use candid::{CandidType, Encode};
use canister_test::Wasm;
use ic_base_types::{CanisterId, PrincipalId};
use ic_management_canister_types::CanisterInstallMode;
use ic_nervous_system_agent::{
    pocketic_impl::PocketIcAgent,
    sns::{swap::SwapCanister, Sns},
    CallCanisters, ProgressNetwork,
};
use ic_nervous_system_common_test_keys::{TEST_NEURON_1_ID, TEST_NEURON_1_OWNER_PRINCIPAL};
use ic_nervous_system_integration_tests::{
    create_service_nervous_system_builder::CreateServiceNervousSystemBuilder,
    nervous_agent_helpers::{
        nns::{ledger::transfer, NnsNeuronAgent},
        sns::swap::{await_swap_lifecycle, participate_in_swap},
    },
    pocket_ic_helpers::{
        await_with_timeout, install_canister_on_subnet,
        sns::{
            governance::{
                propose_to_upgrade_sns_controlled_canister_and_wait,
                EXPECTED_UPGRADE_DURATION_MAX_SECONDS,
            },
            swap::swap_direct_participations,
        },
    },
};
use ic_nns_common::pb::v1::{NeuronId, ProposalId};
use ic_nns_constants::{GOVERNANCE_CANISTER_ID, ROOT_CANISTER_ID};
use ic_nns_governance_api::pb::v1::create_service_nervous_system::SwapParameters;

use ic_sns_governance_api::pb::v1::UpgradeSnsControlledCanister;
use ic_sns_swap::pb::v1::Lifecycle;
use icp_ledger::{AccountIdentifier, Memo, TransferArgs, DEFAULT_TRANSFER_FEE};
use pocket_ic::{management_canister::CanisterStatusResultStatus, nonblocking::PocketIc};

// TODO @rvem: I don't like the fact that this struct definition is copy-pasted from 'canister/canister.rs'.
// We should extract it into a separate crate and reuse in both canister and this crates.
#[derive(CandidType)]
pub struct TestCanisterInitArgs {
    pub greeting: Option<String>,
}

pub async fn install_test_canister(pocket_ic: &PocketIc, args: TestCanisterInitArgs) -> CanisterId {
    let topology = pocket_ic.topology().await;
    let application_subnet_ids = topology.get_app_subnets();
    let application_subnet_id = application_subnet_ids
        .first()
        .expect("No Application subnet found");
    let features = &[];
    let test_canister_wasm =
        Wasm::from_location_specified_by_env_var("sns_testing_canister", features).unwrap();
    install_canister_on_subnet(
        pocket_ic,
        *application_subnet_id,
        Encode!(&args).unwrap(),
        Some(test_canister_wasm),
        vec![ROOT_CANISTER_ID.get()],
    )
    .await
}

pub async fn create_sns_pocket_ic(
    pocket_ic: &PocketIc,
    dapp_canister_ids: Vec<CanisterId>,
) -> (Sns, ProposalId) {
    let neuron_agent = PocketIcAgent::new(pocket_ic, *TEST_NEURON_1_OWNER_PRINCIPAL);
    let nns_neuron_agent = NnsNeuronAgent::new(
        &neuron_agent,
        NeuronId {
            id: TEST_NEURON_1_ID,
        },
    );
    let swap_treasury_agent = PocketIcAgent::new(pocket_ic, GOVERNANCE_CANISTER_ID);
    let swap_partipants_agents = (1..20)
        .into_iter()
        .map(|i| PocketIcAgent::new(pocket_ic, PrincipalId::new_user_test_id(1000 + i as u64)))
        .collect();
    create_sns(
        nns_neuron_agent,
        swap_treasury_agent,
        swap_partipants_agents,
        dapp_canister_ids,
    )
    .await
}

pub async fn create_sns<C: CallCanisters + ProgressNetwork>(
    nns_neuron_agent: NnsNeuronAgent<'_, C>,
    swap_treasury_agent: C,
    swap_participants_agents: Vec<C>,
    dapp_canister_ids: Vec<CanisterId>,
) -> (Sns, ProposalId) {
    let sns_proposal_id = "1";
    let create_service_nervous_system = CreateServiceNervousSystemBuilder::default()
        .neurons_fund_participation(true)
        .with_dapp_canisters(dapp_canister_ids)
        .build();
    let swap_parameters = create_service_nervous_system
        .swap_parameters
        .clone()
        .unwrap();
    assert_eq!(
        swap_parameters.start_time, None,
        "Expecting the swap start time to be None to start the swap immediately"
    );
    let (sns, proposal_id) = nns_neuron_agent
        .propose_to_deploy_sns_and_wait(create_service_nervous_system, sns_proposal_id)
        .await;
    await_swap_lifecycle(&swap_treasury_agent, sns.swap, Lifecycle::Open, true)
        .await
        .expect("Expecting the swap to be open after creation");
    complete_sns_swap(
        &swap_treasury_agent,
        swap_participants_agents,
        swap_parameters,
        sns.swap,
    )
    .await;
    (sns, proposal_id)
}

async fn complete_sns_swap<C: CallCanisters + ProgressNetwork>(
    swap_treasury_agent: &C,
    swap_participants_agents: Vec<C>,
    swap_parameters: SwapParameters,
    swap_canister: SwapCanister,
) {
    let swap_participations = swap_direct_participations(swap_parameters);
    for (swap_participant_amount, swap_participant_agent) in swap_participations
        .iter()
        .zip(swap_participants_agents.iter())
    {
        let transfer_args = TransferArgs {
            to: AccountIdentifier::new(swap_participant_agent.caller().unwrap().into(), None)
                .to_address(),
            amount: (*swap_participant_amount).saturating_add(DEFAULT_TRANSFER_FEE),
            // For now we only mint directly to the participant, so the fee is 0.
            fee: 0.into(),
            memo: Memo(0),
            from_subaccount: None,
            created_at_time: None,
        };
        transfer(swap_treasury_agent, transfer_args).await.unwrap();
        participate_in_swap(
            swap_participant_agent,
            swap_canister,
            *swap_participant_amount,
        )
        .await;
    }
    //smoke_test_participate_and_finalize(pocket_ic, sns.swap.canister_id, swap_parameters).await;
    await_swap_lifecycle(
        swap_treasury_agent,
        swap_canister,
        Lifecycle::Committed,
        true,
    )
    .await
    .expect("Expecting the swap to be commited after creation and swap completion");
}

pub async fn upgrade_sns_controlled_test_canister(
    pocket_ic: &PocketIc,
    sns: Sns,
    canister_id: CanisterId,
    upgrade_arg: TestCanisterInitArgs,
) {
    // For now, we're using the same wasm module, but different init arguments used in 'post_upgrade' hook.
    let features = &[];
    let test_canister_wasm =
        Wasm::from_location_specified_by_env_var("sns_testing_canister", features).unwrap();
    propose_to_upgrade_sns_controlled_canister_and_wait(
        pocket_ic,
        sns.governance.canister_id,
        UpgradeSnsControlledCanister {
            canister_id: Some(canister_id.get()),
            new_canister_wasm: test_canister_wasm.bytes(),
            canister_upgrade_arg: Some(Encode!(&upgrade_arg).unwrap()),
            mode: Some(CanisterInstallMode::Upgrade as i32),
            chunked_canister_wasm: None,
        },
    )
    .await;
    // Wait for the canister to become available
    await_with_timeout(
        pocket_ic,
        0..EXPECTED_UPGRADE_DURATION_MAX_SECONDS,
        |pocket_ic| async {
            let canister_status = pocket_ic
                .canister_status(canister_id.into(), Some(sns.root.canister_id.into()))
                .await;
            canister_status
                .expect("Canister status is unavailable")
                .status as u32
        },
        &(CanisterStatusResultStatus::Running as u32),
    )
    .await
    .expect("Test canister failed to get into the 'Running' state after upgrade");
}
