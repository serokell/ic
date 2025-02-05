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

    pub mod governance {
        use super::*;
        pub async fn manage_neuron<C: CallCanisters>(
            agent: &C,
            neuron_id: NeuronId,
            command: ManageNeuronCommandRequest,
        ) -> ManageNeuronResponse {
            nns_agent::governance::manage_neuron(
                agent,
                ManageNeuronRequest {
                    id: Some(neuron_id),
                    command: Some(command),
                    neuron_id_or_subaccount: None,
                },
            )
            .await
            .unwrap()
        }

        pub async fn make_proposal<C: CallCanisters>(
            agent: &C,
            neuron_id: NeuronId,
            proposal: MakeProposalRequest,
        ) -> ProposalId {
            let command = ManageNeuronCommandRequest::MakeProposal(Box::new(proposal));
            let response = manage_neuron(agent, neuron_id, command).await;
            let response = match response.command {
                Some(Command::MakeProposal(response)) => response,
                _ => panic!("Proposal failed: {:#?}", response),
            };
            response.proposal_id.unwrap_or_else(|| {
                panic!(
                    "Proposal response does not contain a proposal_id: {:#?}",
                    response
                )
            })
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

        pub async fn propose_to_deploy_sns_and_wait<C: CallCanisters + ProgressNetwork>(
            agent: &C,
            neuron_id: NeuronId,
            create_service_nervous_system: CreateServiceNervousSystem,
            sns_instance_label: &str,
        ) -> (Sns, ProposalId) {
            let proposal = MakeProposalRequest {
                title: Some(format!("Create SNS #{}", sns_instance_label)),
                summary: "".to_string(),
                url: "".to_string(),
                action: Some(ProposalActionRequest::CreateServiceNervousSystem(
                    create_service_nervous_system,
                )),
            };
            let nns_proposal_id = make_proposal(agent, neuron_id, proposal).await;
            let _proposal_info = wait_for_proposal_execution(agent, nns_proposal_id)
                .await
                .unwrap();
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
            let sns = Sns::try_from(deployed_sns).expect("Failed to convert DeployedSns to Sns");
            (sns, nns_proposal_id)
        }
    }

    pub mod ledger {
        use super::*;
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
