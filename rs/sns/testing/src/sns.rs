use futures::future::join_all;

use candid::{CandidType, Encode};
use canister_test::Wasm;
use ic_base_types::CanisterId;
use ic_management_canister_types_private::CanisterInstallMode;
use ic_nervous_system_agent::{
    helpers::{
        await_with_timeout,
        nns::propose_to_deploy_sns_and_wait,
        sns::{
            await_swap_lifecycle, get_caller_neuron, get_principal_neurons, participate_in_swap,
            propose, wait_for_proposal_execution,
        },
    },
    nns::{ledger::transfer, sns_wasm::list_deployed_snses},
    sns::{governance::GovernanceCanister, swap::SwapCanister, Sns},
    CallCanisters, ProgressNetwork,
};
use ic_nervous_system_clients::canister_status::CanisterStatusType;
use ic_nervous_system_integration_tests::{
    create_service_nervous_system_builder::CreateServiceNervousSystemBuilder,
    pocket_ic_helpers::sns::{
        governance::EXPECTED_UPGRADE_DURATION_MAX_SECONDS,
        swap::{is_auto_finalization_status_committed_or_err, swap_direct_participations},
    },
};
use ic_nns_common::pb::v1::NeuronId;

use ic_nns_governance_api::pb::v1::create_service_nervous_system::initial_token_distribution::developer_distribution::NeuronDistribution;

use ic_nns_test_utils::common::modify_wasm_bytes;
use ic_sns_governance_api::pb::v1::{
    get_proposal_response::Result as ProposalResult, manage_neuron::Follow, proposal::Action,
    NeuronId as SnsNeuronId, Proposal, ProposalId, UpgradeSnsControlledCanister,
};
use ic_sns_swap::pb::v1::{BuyerState, Lifecycle, TransferableAmount};
use icp_ledger::{AccountIdentifier, Memo, Tokens, TransferArgs, DEFAULT_TRANSFER_FEE};

use crate::utils::{swap_participant_agents, BuildEphemeralAgent, TREASURY_SECRET_KEY};

// TODO @rvem: I don't like the fact that this struct definition is copy-pasted from 'canister/canister.rs'.
// We should extract it into a separate crate and reuse in both canister and this crates.
#[derive(CandidType)]
pub struct TestCanisterInitArgs {
    pub greeting: Option<String>,
}

pub const DEFAULT_SWAP_PARTICIPANTS_NUMBER: usize = 20;

// Creates SNS using agents provided as arguments:
// 1) neuron_agent - agent that controlls 'neuron_id'.
// 2) neuron_id - ID of the neuron that has a sufficient amount of stake to propose the SNS creation and adopt the proposal.
// 2) dev_participant_agent - Agent that will be used as an initial neuron in a newly created SNS. All other
//    neurons will follow the dev neuron.
// 3) swap_treasury_agent - Agent for the identity that has sufficient amout of ICP tokens to close the swap and
//    pay for cycles needed canister upgrade as well as all associated costs.
// 4) swap_participants_agents - Agents for the identities that will participate in the swap. The actual number of participants
//    is determined by the swap parameters. So only some of these agents might be used. Each agent participates in the swap,
//    follows the dev neuron for 'UpgradeSnsControlledCanister' proposals and increases its dissolve delay to be able to vote.
// 5) dapp_canister_ids - Canister IDs of the DApps that will be added to the SNS.
//
// Returns SNS canisters IDs.
pub async fn create_sns<C: CallCanisters + ProgressNetwork + BuildEphemeralAgent>(
    neuron_agent: &C,
    neuron_id: NeuronId,
    dev_participant_agent: &C,
    dapp_canister_ids: Vec<CanisterId>,
    follow_dev_neuron: bool,
) -> Sns {
    let mut create_service_nervous_system = CreateServiceNervousSystemBuilder::default()
        .neurons_fund_participation(true)
        .with_dapp_canisters(dapp_canister_ids)
        .build();
    let governance_parameters = create_service_nervous_system.governance_parameters.clone();

    // Set developer identity to have initial neuron eligible for voting
    create_service_nervous_system.initial_token_distribution = create_service_nervous_system
        .initial_token_distribution
        .map(|mut token_distribution| {
            token_distribution.developer_distribution = token_distribution
                .developer_distribution
                .map(|mut developer_distribution| {
                    developer_distribution.developer_neurons = vec![NeuronDistribution {
                        controller: Some(dev_participant_agent.caller().unwrap().into()),
                        dissolve_delay: governance_parameters
                            .and_then(|p| p.neuron_minimum_dissolve_delay_to_vote),
                        memo: Some(400000),
                        stake: Some(ic_nervous_system_proto::pb::v1::Tokens { e8s: Some(400000) }),
                        vesting_period: Some(ic_nervous_system_proto::pb::v1::Duration::from_secs(
                            0,
                        )),
                    }];
                    developer_distribution
                });
            token_distribution
        });
    let swap_parameters = create_service_nervous_system
        .swap_parameters
        .clone()
        .unwrap();
    assert_eq!(
        swap_parameters.start_time, None,
        "Expecting the swap start time to be None to start the swap immediately"
    );
    let (sns, _proposal_id) = propose_to_deploy_sns_and_wait(
        neuron_agent,
        neuron_id,
        create_service_nervous_system,
        "Create SNS".to_string(),
        "".to_string(),
        "".to_string(),
    )
    .await
    .unwrap();

    let sns_governance = sns.governance;

    let dev_participant_neuron_id = get_caller_neuron(dev_participant_agent, sns_governance)
        .await
        .unwrap()
        .expect("Expecting the identity to have a Neuron");

    let sns_swap = sns.swap;
    complete_sns_swap(
        neuron_agent,
        true,
        sns_swap,
        sns_governance,
        if follow_dev_neuron {
            vec![dev_participant_neuron_id]
        } else {
            vec![]
        },
    )
    .await
    .unwrap();

    sns
}

// Find all SNSes with the given name.
pub async fn find_sns_by_name<C: CallCanisters>(agent: &C, sns_name: String) -> Vec<Sns> {
    let deployed_snses = list_deployed_snses(agent)
        .await
        .expect("Failed to list deployed SNSes");
    let deployed_snses_names = join_all(
        deployed_snses
            .iter()
            .map(|sns| async { sns.governance.metadata(agent).await.map(|r| r.name) }),
    )
    .await;
    deployed_snses_names
        .iter()
        .zip(deployed_snses.iter())
        .filter_map(|(name, sns)| {
            if name
                .as_ref()
                .map(|n| n == &Some(sns_name.clone()))
                .unwrap_or_default()
            {
                Some(sns.clone())
            } else {
                None
            }
        })
        .collect()
}

// helper to get the current participation of the given swap participant
async fn get_current_participation<C: CallCanisters>(
    agent: &C,
    swap_canister: SwapCanister,
) -> u64 {
    let buyer_state = swap_canister
        .get_buyer_state(agent, agent.caller().unwrap().into())
        .await
        .map(|r| r.buyer_state);
    match buyer_state {
        Ok(Some(BuyerState {
            icp: Some(TransferableAmount { amount_e8s, .. }),
            ..
        })) => amount_e8s,
        _ => 0u64,
    }
}

// Completes the swap by transferring the required amount of ICP from the "treasury" account
// and participating in the swap for each participant using agents provided as arguments:
// 1) agent - Agent that is used to provide IC network settings.
// 2) use_ephemeral_icp_treasury - defines whether the identity of the 'agent' or 'TREASURY_PRINCIPAL_ID'.
//    is used to transfer ICP to the swap participants.
// 2) swap_canister - SNS Swap canister ID.
// 3) governance_canister - SNS Governance canister ID.
// 4) neurons_to_follow - SNS Neuron IDs that will be followed by the swap participants.
pub async fn complete_sns_swap<C: CallCanisters + ProgressNetwork + BuildEphemeralAgent>(
    agent: &C,
    use_ephemeral_icp_treasury: bool,
    swap_canister: SwapCanister,
    governance_canister: GovernanceCanister,
    neurons_to_follow: Vec<SnsNeuronId>,
) -> Result<(), String> {
    let swap_treasury_agent = if use_ephemeral_icp_treasury {
        &agent.build_ephemeral_agent(TREASURY_SECRET_KEY.clone())
    } else {
        agent
    };

    println!("Waiting for the swap to be open...");
    await_swap_lifecycle(agent, swap_canister, Lifecycle::Open, true).await?;

    let swap_init = swap_canister
        .get_init(agent)
        .await
        .unwrap()
        .init
        .ok_or("Expecting the swap init to be set")?;
    let minimum_participants = swap_init
        .min_participants
        .ok_or("Expecting the minimum number of participants to be set")?
        as u64;
    let maximum_direct_participation = Tokens::from_e8s(
        swap_init
            .max_direct_participation_icp_e8s
            .ok_or("Expecting the maximum direct participation to be set")?,
    );

    let swap_derived_state = swap_canister
        .get_derived_state(agent)
        .await
        .map_err(|e| format!("Failed to get swap derived state: {e}"))?;

    let direct_participant_count = swap_derived_state.direct_participant_count.unwrap_or(0);
    let direct_participation_icp =
        Tokens::from_e8s(swap_derived_state.direct_participation_icp_e8s.unwrap_or(0));

    // Do exactly one direct participation to close the swap since minimum_participants is already reached.
    let remaining_direct_participation_count = if direct_participant_count >= minimum_participants {
        1
    } else {
        minimum_participants.saturating_sub(direct_participant_count)
    };

    let remaining_direct_participation =
        if direct_participation_icp.get_e8s() >= maximum_direct_participation.get_e8s() {
            println!("Maximum direct participation reached, no more direct participation possible");
            return Ok(());
        } else {
            Tokens::from_e8s(
                maximum_direct_participation
                    .get_e8s()
                    .saturating_sub(direct_participation_icp.get_e8s()),
            )
        };

    println!(
        "Performing {} direct swap participations with cumulative amount of {}",
        remaining_direct_participation_count, remaining_direct_participation
    );
    let swap_participations = swap_direct_participations(
        remaining_direct_participation_count,
        remaining_direct_participation,
    );
    let swap_participants = swap_participant_agents(agent, minimum_participants as usize);
    let mut swap_participants_iter = swap_participants.iter();

    for swap_participant_amount in swap_participations.iter() {
        // Some of the swap_participants might have already participated in the swap,
        // since we need to gather the required number of participants, we skip the ones that already participated.
        let mut swap_participant_agent = swap_participants_iter.next().unwrap();
        while get_current_participation(swap_participant_agent, swap_canister).await != 0 {
            swap_participant_agent = swap_participants_iter.next().unwrap();
        }
        let transfer_args = TransferArgs {
            to: AccountIdentifier::new(swap_participant_agent.caller().unwrap().into(), None)
                .to_address(),
            amount: (*swap_participant_amount).saturating_add(DEFAULT_TRANSFER_FEE),
            fee: DEFAULT_TRANSFER_FEE,
            memo: Memo(0),
            from_subaccount: None,
            created_at_time: None,
        };

        transfer(swap_treasury_agent, transfer_args)
            .await
            .map_err(|e| format!("Failed to transfer ICP to swap participant: {e}"))?
            .map_err(|e| format!("ICP transfer returned an error: {e}"))?;

        participate_in_swap(
            swap_participant_agent,
            swap_canister,
            *swap_participant_amount,
            swap_init.confirmation_text.clone(),
        )
        .await?;
    }

    println!("Waiting for the swap to be completed...");
    await_swap_lifecycle(agent, swap_canister, Lifecycle::Committed, true).await?;
    await_with_timeout(
        agent,
        0..EXPECTED_UPGRADE_DURATION_MAX_SECONDS,
        |agent| async {
            let auto_finalization_status = swap_canister
                .get_auto_finalization_status(agent)
                .await
                .map_err(|e| format!("Failed to get auto finalization status: {e}"))?;
            is_auto_finalization_status_committed_or_err(&auto_finalization_status)
        },
        &Ok(true),
    )
    .await?;

    let sns_nervous_system_parameters = governance_canister
        .get_nervous_system_parameters(agent)
        .await
        .map_err(|e| format!("Failed to get nervous system parameters: {e}"))?;
    let neuron_mininum_disolve_delay = sns_nervous_system_parameters
        .neuron_minimum_dissolve_delay_to_vote_seconds
        .ok_or("Expecting the neuron minimum dissolve delay to be set")?
        as u32;

    println!(
        "Increasing dissolve delay to {} for swap participants...",
        neuron_mininum_disolve_delay
    );
    for swap_participant_agent in swap_participants {
        let swap_participant_neuron_id =
            get_caller_neuron(&swap_participant_agent, governance_canister)
                .await
                .map_err(|e| {
                    format!(
                        "Failed to get the caller neuron for {}: {e}",
                        swap_participant_agent.caller().unwrap()
                    )
                })?;
        if let Some(swap_participant_neuron_id) = swap_participant_neuron_id {
            for neuron_to_follow in &neurons_to_follow {
                let follow = Follow {
                    followees: vec![neuron_to_follow.clone()],
                    // UpgradeSnsControlledCanister
                    // TODO: @rvem: Do we need to enable other functions following too?
                    function_id: 3,
                };
                governance_canister
                    .follow(
                        &swap_participant_agent,
                        swap_participant_neuron_id.clone(),
                        follow,
                    )
                    .await
                    .map_err(|e| {
                        format!(
                            "Neuron {:?} failed to follow the neuron: {}",
                            swap_participant_neuron_id.clone(),
                            e
                        )
                    })?;
            }

            governance_canister
                .increase_dissolve_delay(
                    &swap_participant_agent,
                    swap_participant_neuron_id.clone(),
                    neuron_mininum_disolve_delay,
                )
                .await
                .map_err(|e| {
                    format!(
                        "Failed to increase dissolve delay for neuron {:?}: {}",
                        swap_participant_neuron_id, e
                    )
                })?;
        }
    }

    Ok(())
}

pub async fn sns_proposal_upvote<C: CallCanisters + BuildEphemeralAgent + ProgressNetwork>(
    agent: &C,
    governance_canister: GovernanceCanister,
    // Swap canister is needed to determine the number of direct participants.
    swap_canister: SwapCanister,
    proposal_id: u64,
    wait: bool,
) -> Result<(), String> {
    let proposal_id = ProposalId { id: proposal_id };
    let proposal_info = governance_canister
        .get_proposal(agent, proposal_id)
        .await
        .map_err(|e| format!("Failed to get the proposal: {e}"))?;

    match proposal_info
        .result
        .ok_or("Expecting the proposal result to be set")?
    {
        ProposalResult::Proposal(proposal_data) => {
            if proposal_data.decided_timestamp_seconds > 0 {
                return Err("The proposal was already decided".to_string());
            }
        }
        ProposalResult::Error(e) => {
            return Err(format!("Getting proposal returned a governance error: {e}"));
        }
    }

    let swap_derived_state = swap_canister
        .get_derived_state(agent)
        .await
        .map_err(|e| format!("Failed to get swap derived state: {e}"))?;

    let direct_participant_count = swap_derived_state.direct_participant_count.unwrap_or(0);
    // Our assumption is that there are at most 'direct_participant_count' known identities that participated
    // in the swap within 'complete_sns_swap' function previously.
    // We will use these identities to upvote the proposal.
    let vote_participant_agents = swap_participant_agents(agent, direct_participant_count as usize);
    for vote_participant_agent in vote_participant_agents {
        let vote_participant_neurons = get_principal_neurons(
            agent,
            governance_canister,
            vote_participant_agent.caller().unwrap().into(),
        )
        .await
        .map_err(|e| format!("Failed to get principal neurons: {e}"))?;
        if let Some(neuron) = vote_participant_neurons.first() {
            governance_canister
                .register_vote(&vote_participant_agent, neuron.clone(), proposal_id, 1)
                .await
                .map_err(|e| format!("Failed to upvote the proposal: {e}"))?;
        }
    }
    if wait {
        println!("Waiting for the proposal to be executed...");
        wait_for_proposal_execution(agent, governance_canister, proposal_id)
            .await
            .map_err(|e| format!("Failed to wait for proposal execution: {e}"))?;
    }
    Ok(())
}

// Upgrades the test canister controlled by the SNS using arguments:
// 1) dev_participant_agent - Agent for the identity that will be used to submit the proposal to upgrade the canister.
//    It is expected that neuron associated with this identity has sufficient amount of voting power to adopt the proposal
//    or it is followed by sufficient number of other neurons to have the proposal adopted using their voting power.
// 2) sns - SNS canisters.
// 3) canister_id - ID of the canister that will be upgraded.
// 4) upgrade_arg - Arguments that will be passed to the canister during the upgrade.
pub async fn propose_sns_controlled_test_canister_upgrade<C: CallCanisters + ProgressNetwork>(
    dev_participant_agent: &C,
    sns: Sns,
    canister_id: CanisterId,
    upgrade_arg: TestCanisterInitArgs,
) -> ProposalId {
    // For now, we're using the same wasm module, but different init arguments used in 'post_upgrade' hook.
    let features = &[];
    let test_canister_wasm =
        Wasm::from_location_specified_by_env_var("sns_testing_canister", features).unwrap();
    let modified_test_canister_wasm = modify_wasm_bytes(&test_canister_wasm.bytes(), 42);

    // TODO: @rvem: It's impossible to use 'upgrade_sns_controlled_canister::exec' function to upgrade the canister
    // using the ic-agent backend on the network run by PocketIC because pocket-ic-server currently doesn't support
    // calls to the management canister ('aaaaa-aa'), hence for now the upgrade is done using a single 'manage_neuron'
    // call to the governance canister.
    // let temp_dir = TempDir::new().unwrap();
    // let new_wasm_path = temp_dir.path().join("new_test_canister.wasm");
    // {
    //     let mut new_wasm_file = File::create(&new_wasm_path).unwrap();
    //     new_wasm_file
    //         .write_all(&modified_test_canister_wasm)
    //         .unwrap();
    //     new_wasm_file.flush().unwrap();
    // }

    // let icp = Tokens::from_tokens(10).unwrap();
    // convert_icp_to_cycles(dev_participant_agent, icp).await;

    // let neuron_id = get_caller_neuron(dev_participant_agent, sns.governance)
    //     .await
    //     .unwrap();
    // let candid_arg = (candid::IDLArgs {
    //     args: vec![candid::IDLValue::try_from_candid_type(&upgrade_arg).unwrap()],
    // })
    // .to_string();
    // let upgrade_args = UpgradeSnsControlledCanisterArgs {
    //     sns_neuron_id: Some(ParsedSnsNeuron(neuron_id)),
    //     target_canister_id: canister_id,
    //     wasm_path: new_wasm_path,
    //     candid_arg: Some(candid_arg),
    //     proposal_url: Url::try_from("https://github.com/dfinity/ic").unwrap(),
    //     summary: "Upgrade Test canister".to_string(),
    // };
    // let UpgradeSnsControlledCanisterInfo { proposal_id, .. } =
    //     upgrade_sns_controlled_canister::exec(upgrade_args, dev_participant_agent)
    //         .await
    //         .expect("Failed to upgrade the canister");
    // let proposal_id = proposal_id.unwrap();

    let neuron_id = get_caller_neuron(dev_participant_agent, sns.governance)
        .await
        .unwrap()
        .expect("Expecting the identity to have a Neuron");
    propose(
        dev_participant_agent,
        neuron_id,
        sns.governance,
        Proposal {
            title: "Upgrade SNS controlled canister.".to_string(),
            summary: "".to_string(),
            url: "".to_string(),
            action: Some(Action::UpgradeSnsControlledCanister(
                UpgradeSnsControlledCanister {
                    canister_id: Some(canister_id.get()),
                    new_canister_wasm: modified_test_canister_wasm,
                    canister_upgrade_arg: Some(Encode!(&upgrade_arg).unwrap()),
                    mode: Some(CanisterInstallMode::Upgrade as i32),
                    chunked_canister_wasm: None,
                },
            )),
        },
    )
    .await
    .unwrap()
}

// Waits for the upgrade proposal to be adopted and executed and then waits for the canister to become available
// after upgrade using arguments:
// 1) agent - Agent that will be used to check the status of the canister.
// 2) proposal_id - ID of the proposal that will be waited for.
// 3) canister_id - ID of the canister that receives an upgrade.
// 4) sns - SNS canisters.
pub async fn wait_for_sns_controlled_canister_upgrade<C: CallCanisters + ProgressNetwork>(
    agent: &C,
    proposal_id: ProposalId,
    canister_id: CanisterId,
    sns: Sns,
) {
    wait_for_proposal_execution(agent, sns.governance, proposal_id)
        .await
        .expect("Failed to execute the proposal");

    // Wait for the canister to become available
    await_with_timeout(
        agent,
        0..EXPECTED_UPGRADE_DURATION_MAX_SECONDS,
        |agent: &C| async {
            let canister_status = sns
                .root
                .get_sns_controlled_canister_status(agent, canister_id)
                .await
                .map(|result| result.status);
            canister_status
                .map(|status| status == CanisterStatusType::Running)
                .unwrap_or_default()
        },
        &true,
    )
    .await
    .expect("Test canister failed to get into the 'Running' state after upgrade");
}

// Module with PocketIC-specific functions, mainly used in the tests.
pub mod pocket_ic {
    use super::TestCanisterInitArgs;

    use ::pocket_ic::nonblocking::PocketIc;
    use candid::Encode;
    use canister_test::Wasm;
    use ic_base_types::{CanisterId, PrincipalId};
    use ic_nervous_system_agent::{pocketic_impl::PocketIcAgent, sns::Sns};
    use ic_nervous_system_integration_tests::pocket_ic_helpers::{
        install_canister_on_subnet, nns::ledger::mint_icp,
    };
    use ic_nns_common::pb::v1::NeuronId;
    use ic_nns_constants::ROOT_CANISTER_ID;
    use ic_sns_governance_api::pb::v1::ProposalId;
    use icp_ledger::{Tokens, DEFAULT_TRANSFER_FEE};

    pub async fn install_test_canister(
        pocket_ic: &PocketIc,
        args: TestCanisterInitArgs,
    ) -> CanisterId {
        let topology = pocket_ic.topology().await;
        let application_subnet_ids = topology.get_app_subnets();
        let application_subnet_id = application_subnet_ids[0];
        let features = &[];
        let test_canister_wasm =
            Wasm::from_location_specified_by_env_var("sns_testing_canister", features).unwrap();
        install_canister_on_subnet(
            pocket_ic,
            application_subnet_id,
            Encode!(&args).unwrap(),
            Some(test_canister_wasm),
            vec![ROOT_CANISTER_ID.get()],
        )
        .await
    }

    // PocketIC-specific version of 'create_sns' function.
    // Takes the list of IDs of the DApps that will be added to the SNS as an argument.
    //
    // Returns SNS canisters IDs and the dev participant ID.
    pub async fn create_sns(
        pocket_ic: &PocketIc,
        dev_participant_id: PrincipalId,
        dev_neuron_id: NeuronId,
        dapp_canister_ids: Vec<CanisterId>,
        follow_dev_neuron: bool,
    ) -> Sns {
        let dev_participant = PocketIcAgent::new(pocket_ic, dev_participant_id);

        super::create_sns(
            &dev_participant,
            dev_neuron_id,
            &dev_participant,
            dapp_canister_ids,
            follow_dev_neuron,
        )
        .await
    }

    // PocketIC-specific version of 'upgrade_sns_controlled_test_canister' function.
    // Upgrades the test canister controlled by the SNS using arguments:
    // 1) pocket_ic - PocketIC instance.
    // 2) dev_participant_id - ID of the identity that will be used to submit the proposal to upgrade the canister.
    //    It is expected that neuron associated with this identity has sufficient amount of voting power to adopt the proposal
    //    or it is followed by sufficient number of other neurons to have the proposal adopted using their voting power.
    // 3) sns - SNS canisters.
    // 4) canister_id - ID of the canister that will be upgraded.
    // 5) upgrade_arg - Arguments that will be passed to the canister during the upgrade.
    pub async fn propose_sns_controlled_test_canister_upgrade(
        pocket_ic: &PocketIc,
        dev_participant_id: PrincipalId,
        sns: Sns,
        canister_id: CanisterId,
        upgrade_arg: TestCanisterInitArgs,
    ) -> ProposalId {
        let dev_participant_agent = PocketIcAgent::new(pocket_ic, dev_participant_id);
        mint_icp(
            pocket_ic,
            dev_participant_id.into(),
            Tokens::from_tokens(10_u64)
                .unwrap()
                .saturating_add(DEFAULT_TRANSFER_FEE),
            None,
        )
        .await;

        super::propose_sns_controlled_test_canister_upgrade(
            &dev_participant_agent,
            sns,
            canister_id,
            upgrade_arg,
        )
        .await
    }
    pub async fn wait_for_sns_controlled_canister_upgrade(
        pocket_ic: &PocketIc,
        proposal_id: ProposalId,
        canister_id: CanisterId,
        sns: Sns,
    ) {
        super::wait_for_sns_controlled_canister_upgrade(pocket_ic, proposal_id, canister_id, sns)
            .await
    }
}
