use crate::nns::governance::requests::{GetNetworkEconomicsParameters, GetProposalInfo};
use crate::CallCanisters;
use ic_nns_common::pb::v1::ProposalId;
use ic_nns_constants::GOVERNANCE_CANISTER_ID;
use ic_nns_governance_api::pb::v1::{
    GetNeuronsFundAuditInfoRequest, GetNeuronsFundAuditInfoResponse, ListNeurons,
    ListNeuronsResponse, ManageNeuronRequest, ManageNeuronResponse, ProposalInfo,
};

pub mod requests;

pub async fn get_neurons_fund_audit_info<C: CallCanisters>(
    agent: &C,
    nns_proposal_id: ProposalId,
) -> Result<GetNeuronsFundAuditInfoResponse, C::Error> {
    let request = GetNeuronsFundAuditInfoRequest {
        nns_proposal_id: Some(nns_proposal_id),
    };
    agent.call(GOVERNANCE_CANISTER_ID, request).await
}

pub async fn list_neurons<C: CallCanisters>(
    agent: &C,
    list_neurons: ListNeurons,
) -> Result<ListNeuronsResponse, C::Error> {
    agent.call(GOVERNANCE_CANISTER_ID, list_neurons).await
}

pub async fn manage_neuron<C: CallCanisters>(
    agent: &C,
    manage_neuron: ManageNeuronRequest,
) -> Result<ManageNeuronResponse, C::Error> {
    agent.call(GOVERNANCE_CANISTER_ID, manage_neuron).await
}

pub async fn get_proposal_info<C: CallCanisters>(
    agent: &C,
    proposal_id: ProposalId,
) -> Result<Option<ProposalInfo>, C::Error> {
    agent
        .call(GOVERNANCE_CANISTER_ID, GetProposalInfo(proposal_id))
        .await
}

pub async fn get_network_economics_parameters<C: CallCanisters>(
    agent: &C,
) -> Result<ic_nns_governance_api::pb::v1::NetworkEconomics, C::Error> {
    agent
        .call(GOVERNANCE_CANISTER_ID, GetNetworkEconomicsParameters())
        .await
}
