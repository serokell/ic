use assert_matches::assert_matches;
use candid::Nat;
use ic_base_types::PrincipalId;
use ic_ledger_core::Tokens;
use ic_nervous_system_agent::sns::Sns;
use ic_nervous_system_agent::{CallCanisters, ProgressNetwork};
use std::time::Duration;

pub mod nns {
    use super::*;
    use ic_nervous_system_agent::nns as nns_agent;
    use ic_nns_common::pb::v1::{NeuronId, ProposalId};
    use ic_nns_governance_api::pb::v1::{
        manage_neuron_response::Command, CreateServiceNervousSystem, MakeProposalRequest,
        ManageNeuronCommandRequest, ManageNeuronRequest, ManageNeuronResponse,
        ProposalActionRequest, ProposalInfo, Topic,
    };
    use ic_sns_wasm::pb::v1::get_deployed_sns_by_proposal_id_response::GetDeployedSnsByProposalIdResult;
    use ic_sns_wasm::pb::v1::GetDeployedSnsByProposalIdResponse;

    // A wrapper around ic-nervous-system-agent that specifies a neuron for
    // the NNS governance requests.
    pub struct NnsNeuronAgent<'a, C: CallCanisters> {
        pub agent: &'a C,
        pub neuron_id: NeuronId,
    }

    impl<'a, C: CallCanisters> NnsNeuronAgent<'a, C> {
        pub fn new(agent: &'a C, neuron_id: NeuronId) -> Self {
            Self { agent, neuron_id }
        }
    }

    pub mod governance {
        use super::*;

        impl<'a, C: CallCanisters + ProgressNetwork> NnsNeuronAgent<'a, C> {
            pub async fn manage_neuron(
                self,
                command: ManageNeuronCommandRequest,
            ) -> ManageNeuronResponse {
                nns_agent::governance::manage_neuron(
                    self.agent,
                    ManageNeuronRequest {
                        id: Some(self.neuron_id),
                        command: Some(command),
                        neuron_id_or_subaccount: None,
                    },
                )
                .await
                .unwrap()
            }

            pub async fn propose_and_wait(
                self,
                proposal: MakeProposalRequest,
            ) -> Result<ProposalInfo, String> {
                let agent = self.agent;
                let command = ManageNeuronCommandRequest::MakeProposal(Box::new(proposal));
                let response = self.manage_neuron(command).await;
                let response = match response.command {
                    Some(Command::MakeProposal(response)) => response,
                    _ => panic!("Proposal failed: {:#?}", response),
                };
                let proposal_id = response.proposal_id.unwrap_or_else(|| {
                    panic!(
                        "Proposal response does not contain a proposal_id: {:#?}",
                        response
                    )
                });
                wait_for_proposal_execution(agent, proposal_id).await
            }
            pub async fn propose_to_deploy_sns_and_wait(
                self,
                create_service_nervous_system: CreateServiceNervousSystem,
                sns_instance_label: &str,
            ) -> (Sns, ProposalId) {
                let agent = self.agent;
                let proposal = MakeProposalRequest {
                    title: Some(format!("Create SNS #{}", sns_instance_label)),
                    summary: "".to_string(),
                    url: "".to_string(),
                    action: Some(ProposalActionRequest::CreateServiceNervousSystem(
                        create_service_nervous_system,
                    )),
                };
                let proposal_info = self.propose_and_wait(proposal).await.unwrap();
                let nns_proposal_id = proposal_info.id.unwrap();
                let Some(GetDeployedSnsByProposalIdResult::DeployedSns(deployed_sns)) =
                    sns_wasm::get_deployed_sns_by_proposal_id(agent, nns_proposal_id)
                        .await
                        .get_deployed_sns_by_proposal_id_result
                else {
                    panic!(
                        "NNS proposal {:?} did not result in a successfully deployed SNS {}.",
                        nns_proposal_id, sns_instance_label,
                    );
                };
                let sns =
                    Sns::try_from(deployed_sns).expect("Failed to convert DeployedSns to Sns");
                (sns, nns_proposal_id)
            }
        }

        pub async fn wait_for_proposal_execution<C: CallCanisters + ProgressNetwork>(
            agent: &C,
            proposal_id: ProposalId,
        ) -> Result<ProposalInfo, String> {
            // We progress the network until the proposal is executed
            let mut last_proposal_info = None;
            for _attempt_count in 1..=100 {
                agent.progress(Duration::from_secs(1)).await;
                let proposal_info_result = nns_get_proposal_info(agent, proposal_id).await;

                let proposal_info = match proposal_info_result {
                    Ok(proposal_info) => proposal_info,
                    Err(user_error) => {
                        // Upgrading NNS Governance results in the proposal info temporarily not
                        // being available due to the canister being stopped. This requires
                        // more attempts to get the proposal info to find out if the proposal
                        // actually got executed.
                        if agent.is_canister_stopped_error(&user_error) {
                            continue;
                        } else {
                            return Err(format!("Error getting proposal info: {:#?}", user_error));
                        }
                    }
                };

                if proposal_info.executed_timestamp_seconds > 0 {
                    return Ok(proposal_info);
                }
                assert_eq!(
                    proposal_info.failure_reason,
                    None,
                    "Execution failed for {:?} proposal '{}': {:#?}",
                    Topic::try_from(proposal_info.topic).unwrap(),
                    proposal_info
                        .proposal
                        .unwrap()
                        .title
                        .unwrap_or("<no-title>".to_string()),
                    proposal_info.failure_reason
                );
                last_proposal_info = Some(proposal_info);
            }
            Err(format!(
                "Looks like proposal {:?} is never going to be executed: {:#?}",
                proposal_id, last_proposal_info,
            ))
        }

        pub async fn nns_get_proposal_info<C: CallCanisters>(
            agent: &C,
            proposal_id: ProposalId,
        ) -> Result<ProposalInfo, C::Error> {
            nns_agent::governance::get_proposal_info(agent, proposal_id)
                .await
                .map(|result| result.unwrap())
        }
    }

    pub mod ledger {
        use super::*;
        use ic_ledger_core::Tokens;
        use icp_ledger::{AccountIdentifier, BinaryAccountBalanceArgs, TransferArgs};
        use icrc_ledger_types::icrc1::transfer::TransferArg;

        pub async fn icrc1_transfer<C: CallCanisters>(
            agent: &C,
            transfer_arg: TransferArg,
        ) -> Result<Nat, icrc_ledger_types::icrc1::transfer::TransferError> {
            nns_agent::ledger::icrc1_transfer(agent, transfer_arg)
                .await
                .unwrap()
        }

        pub async fn account_balance<C: CallCanisters>(
            agent: &C,
            account: &AccountIdentifier,
        ) -> Tokens {
            nns_agent::ledger::account_balance(
                agent,
                BinaryAccountBalanceArgs {
                    account: account.to_address(),
                },
            )
            .await
            .unwrap()
        }

        pub async fn transfer<C: CallCanisters>(
            agent: &C,
            transfer_args: TransferArgs,
        ) -> Result<u64, icp_ledger::TransferError> {
            nns_agent::ledger::transfer(agent, transfer_args)
                .await
                .unwrap()
        }
    }

    pub mod sns_wasm {
        use super::*;
        pub async fn get_deployed_sns_by_proposal_id<C: CallCanisters>(
            agent: &C,
            proposal_id: ProposalId,
        ) -> GetDeployedSnsByProposalIdResponse {
            nns_agent::sns_wasm::get_deployed_sns_by_proposal_id(agent, proposal_id)
                .await
                .unwrap()
        }
    }
}

pub mod sns {
    use super::*;
    use ic_nervous_system_agent::sns::governance::{
        GovernanceCanister, ProposalSubmissionError, SubmittedProposal,
    };
    use ic_nervous_system_agent::sns::swap::SwapCanister;
    use ic_sns_governance_api::pb::v1::{
        get_proposal_response, manage_neuron::Command, GetProposalResponse, GovernanceError,
        ManageNeuronResponse, NeuronId, Proposal, ProposalData, ProposalId,
    };
    use ic_sns_swap::{
        pb::v1::{
            BuyerState, GetBuyerStateRequest, GetBuyerStateResponse, Lifecycle,
            RefreshBuyerTokensRequest, RefreshBuyerTokensResponse,
        },
        swap::principal_to_subaccount,
    };

    // A wrapper around ic-nervous-system-agent that specifies a neuron for
    // the NNS governance requests.
    pub struct SnsNeuronAgent<'a, C: CallCanisters> {
        pub agent: &'a C,
        pub neuron_id: NeuronId,
        pub sns_governance_canister: GovernanceCanister,
    }

    impl<'a, C: CallCanisters> SnsNeuronAgent<'a, C> {
        pub fn new(
            agent: &'a C,
            neuron_id: NeuronId,
            sns_governance_canister_id: PrincipalId,
        ) -> Self {
            Self {
                agent,
                neuron_id,
                sns_governance_canister: GovernanceCanister::new(sns_governance_canister_id),
            }
        }
    }

    pub mod governance {
        use super::*;

        impl<'a, C: CallCanisters + ProgressNetwork> SnsNeuronAgent<'a, C> {
            pub async fn manage_neuron(self, command: Command) -> ManageNeuronResponse {
                self.sns_governance_canister
                    .manage_neuron(self.agent, self.neuron_id, command)
                    .await
                    .unwrap()
            }

            pub async fn propose_and_wait(
                self,
                proposal: Proposal,
            ) -> Result<ProposalData, GovernanceError> {
                let agent = self.agent;
                let response = self
                    .sns_governance_canister
                    .submit_proposal(agent, self.neuron_id, proposal)
                    .await
                    .unwrap();
                let SubmittedProposal { proposal_id } = SubmittedProposal::try_from(response)
                    .map_err(|err| match err {
                        ProposalSubmissionError::GovernanceError(e) => e,
                        e => panic!("Unexpected error: {e}"),
                    })?;

                wait_for_proposal_execution(agent, self.sns_governance_canister, proposal_id).await
            }
        }

        pub async fn wait_for_proposal_execution<C: CallCanisters + ProgressNetwork>(
            agent: &C,
            sns_governance_canister: GovernanceCanister,
            proposal_id: ProposalId,
        ) -> Result<ProposalData, GovernanceError> {
            // We create some blocks until the proposal has finished executing (`pocket_ic.tick()`).
            let mut last_proposal_data = None;
            for _attempt_count in 1..=50 {
                agent.progress(Duration::from_secs(1)).await;
                let proposal_result =
                    get_proposal(agent, sns_governance_canister, proposal_id).await;

                let proposal = match proposal_result {
                    Ok(proposal) => proposal,
                    Err(user_error) => {
                        // Upgrading SNS Governance results in the proposal info temporarily not
                        // being available due to the canister being stopped. This requires
                        // more attempts to get the proposal info to find out if the proposal
                        // actually got executed.
                        if agent.is_canister_stopped_error(&user_error) {
                            continue;
                        } else {
                            panic!("Error getting proposal: {:#?}", user_error);
                        }
                    }
                };

                let proposal = proposal
                    .result
                    .expect("GetProposalResponse.result must be set.");
                let proposal_data = match proposal {
                    get_proposal_response::Result::Error(err) => {
                        panic!("Proposal data cannot be found: {:?}", err);
                    }
                    get_proposal_response::Result::Proposal(proposal_data) => proposal_data,
                };
                if proposal_data.executed_timestamp_seconds > 0 {
                    return Ok(proposal_data);
                }
                proposal_data.failure_reason.clone().map_or(Ok(()), Err)?;
                last_proposal_data = Some(proposal_data);
            }
            panic!(
                "Looks like the SNS proposal {:?} is never going to be decided: {:#?}",
                proposal_id, last_proposal_data
            );
        }

        pub async fn get_proposal<C: CallCanisters>(
            agent: &C,
            governance_canister: GovernanceCanister,
            proposal_id: ProposalId,
        ) -> Result<GetProposalResponse, C::Error> {
            governance_canister.get_proposal(agent, proposal_id).await
        }
    }

    pub mod swap {
        use super::*;
        use icrc_ledger_types::icrc1::{account::Account, transfer::TransferArg};

        pub async fn refresh_buyer_tokens<C: CallCanisters>(
            agent: &C,
            sns_swap_canister: SwapCanister,
            buyer: PrincipalId,
            confirmation_text: Option<String>,
        ) -> Result<RefreshBuyerTokensResponse, String> {
            sns_swap_canister
                .refresh_buyer_tokens(
                    agent,
                    RefreshBuyerTokensRequest {
                        buyer: buyer.to_string(),
                        confirmation_text,
                    },
                )
                .await
                .map_err(|err| err.to_string())
        }

        pub async fn get_buyer_state<C: CallCanisters>(
            agent: &C,
            sns_swap_canister: SwapCanister,
            buyer: PrincipalId,
        ) -> Result<GetBuyerStateResponse, String> {
            sns_swap_canister
                .get_buyer_state(
                    agent,
                    GetBuyerStateRequest {
                        principal_id: Some(buyer),
                    },
                )
                .await
                .map_err(|err| err.to_string())
        }

        pub async fn await_swap_lifecycle<C: CallCanisters + ProgressNetwork>(
            agent: &C,
            sns_swap_canister: SwapCanister,
            expected_lifecycle: Lifecycle,
            swap_immediately_open: bool,
        ) -> Result<(), String> {
            // The swap opens in up to 48 after the proposal for creating this SNS was executed
            // if non-test version of NNS Governance canister is used.
            if !swap_immediately_open {
                agent.progress(Duration::from_secs(48 * 60 * 60)).await;
            }
            let mut last_lifecycle = None;
            for _attempt_count in 1..=100 {
                agent.progress(Duration::from_secs(1)).await;
                let response = sns_swap_canister.get_lifecycle(agent).await.unwrap();
                let lifecycle = Lifecycle::try_from(response.lifecycle.unwrap()).unwrap();
                if lifecycle == expected_lifecycle {
                    return Ok(());
                }
                last_lifecycle = Some(lifecycle);
            }
            Err(format!(
                "Looks like the SNS lifecycle {:?} is never going to be reached: {:?}",
                expected_lifecycle, last_lifecycle,
            ))
        }

        pub async fn participate_in_swap<C: CallCanisters>(
            agent: &C,
            sns_swap_canister: SwapCanister,
            amount_icp_excluding_fees: Tokens,
        ) {
            let direct_participant = agent.caller().unwrap().into();
            let direct_participant_swap_subaccount =
                Some(principal_to_subaccount(&direct_participant));

            let direct_participant_swap_account = Account {
                owner: sns_swap_canister.canister_id.0,
                subaccount: direct_participant_swap_subaccount,
            };

            let participation_amount = amount_icp_excluding_fees.get_e8s();
            nns::ledger::icrc1_transfer(
                agent,
                TransferArg {
                    from_subaccount: None,
                    to: direct_participant_swap_account,
                    fee: None,
                    created_at_time: None,
                    memo: None,
                    amount: Nat::from(participation_amount),
                },
            )
            .await
            .unwrap();

            let response =
                refresh_buyer_tokens(agent, sns_swap_canister, direct_participant, None).await;

            assert_eq!(
                response,
                Ok(RefreshBuyerTokensResponse {
                    icp_ledger_account_balance_e8s: amount_icp_excluding_fees.get_e8s(),
                    icp_accepted_participation_e8s: amount_icp_excluding_fees.get_e8s(),
                })
            );

            let response = get_buyer_state(agent, sns_swap_canister, direct_participant)
                .await
                .expect("Swap.get_buyer_state response should be Ok.");
            let (icp, has_created_neuron_recipes) = assert_matches!(
                response.buyer_state,
                Some(BuyerState {
                    icp,
                    has_created_neuron_recipes,
                }) => (
                    icp.expect("buyer_state.icp must be specified."),
                    has_created_neuron_recipes
                        .expect("buyer_state.has_created_neuron_recipes must be specified.")
                )
            );
            assert!(
                !has_created_neuron_recipes,
                "Neuron recipes are expected to be created only after the swap is adopted"
            );
            assert_eq!(icp.amount_e8s, amount_icp_excluding_fees.get_e8s());
        }
    }
}
