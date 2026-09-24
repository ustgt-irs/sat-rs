use std::collections::HashMap;
use std::sync::mpsc;

use log::warn;
use satrs::action::ActionRequest;
use satrs::hk::HkRequest;
use satrs::mode::ModeRequest;
use satrs::pus::verification::{
    FailParams, TcStateAccepted, VerificationReportingProvider, VerificationToken,
};
use satrs::pus::{ActiveRequest, EcssTmSender, GenericRoutingError, PusRequestRouter};
use satrs::queue::GenericSendError;
use satrs::request::{GenericMessage, MessageMetadata, UniqueApidTargetId};
use satrs::spacepackets::ecss::tc::PusTcReader;
use satrs::spacepackets::ecss::PusPacket;
use satrs::ComponentId;
use satrs_example::config::tmtc_err;

/*
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum CompositeRequest {
    Hk(HkRequest),
    Action(ActionRequest),
}

#[derive(Clone)]
pub struct GenericRequestRouter {
    #[allow(dead_code)]
    pub id: ComponentId,
    // All messages which do not have a dedicated queue.
    pub composite_router_map:
        HashMap<ComponentId, mpsc::SyncSender<GenericMessage<CompositeRequest>>>,
    pub mode_router_map: HashMap<ComponentId, mpsc::SyncSender<GenericMessage<ModeRequest>>>,
}

impl Default for GenericRequestRouter {
    fn default() -> Self {
        Self {
            id: ids::generic_pus::PUS_ROUTING.raw(),
            composite_router_map: Default::default(),
            mode_router_map: Default::default(),
        }
    }
}
impl GenericRequestRouter {
    pub(crate) fn handle_error_generic(
        &self,
        active_request: &impl ActiveRequest,
        tc: &PusTcReader,
        error: GenericRoutingError,
        tm_sender: &(impl EcssTmSender + ?Sized),
        verif_reporter: &impl VerificationReportingProvider,
        time_stamp: &[u8],
    ) {
        warn!(
            "Routing request for service {} failed: {error:?}",
            tc.service_type_id()
        );
        let accepted_token: VerificationToken<TcStateAccepted> = active_request
            .token()
            .try_into()
            .expect("token is not in accepted state");
        match error {
            GenericRoutingError::UnknownTargetId(id) => {
                let apid_target_id = UniqueApidTargetId::from(id);
                warn!("Target APID for request: {}", apid_target_id.apid);
                warn!("Target Unique ID for request: {}", apid_target_id.unique_id);
                let mut fail_data: [u8; core::mem::size_of::<ComponentId>()] =
                    [0; core::mem::size_of::<ComponentId>()];
                fail_data.copy_from_slice(&id.to_be_bytes());
                verif_reporter
                    .completion_failure(
                        tm_sender,
                        accepted_token,
                        FailParams::new(time_stamp, &tmtc_err::UNKNOWN_TARGET_ID, &fail_data),
                    )
                    .expect("Sending start failure failed");
            }
            GenericRoutingError::Send(_) => {
                let mut fail_data: [u8; 8] = [0; 8];
                fail_data.copy_from_slice(&active_request.target_id().to_be_bytes());
                verif_reporter
                    .completion_failure(
                        tm_sender,
                        accepted_token,
                        FailParams::new(time_stamp, &tmtc_err::ROUTING_ERROR, &fail_data),
                    )
                    .expect("Sending start failure failed");
            }
        }
    }
}
impl PusRequestRouter<HkRequest> for GenericRequestRouter {
    type Error = GenericRoutingError;

    fn route(
        &self,
        requestor_info: MessageMetadata,
        target_id: ComponentId,
        hk_request: HkRequest,
    ) -> Result<(), Self::Error> {
        if let Some(sender) = self.composite_router_map.get(&target_id) {
            sender
                .send(GenericMessage::new(
                    requestor_info,
                    CompositeRequest::Hk(hk_request),
                ))
                .map_err(|_| GenericRoutingError::Send(GenericSendError::RxDisconnected))?;
            return Ok(());
        }
        Err(GenericRoutingError::UnknownTargetId(target_id))
    }
}

impl PusRequestRouter<ActionRequest> for GenericRequestRouter {
    type Error = GenericRoutingError;

    fn route(
        &self,
        requestor_info: MessageMetadata,
        target_id: ComponentId,
        action_request: ActionRequest,
    ) -> Result<(), Self::Error> {
        if let Some(sender) = self.composite_router_map.get(&target_id) {
            sender
                .send(GenericMessage::new(
                    requestor_info,
                    CompositeRequest::Action(action_request),
                ))
                .map_err(|_| GenericRoutingError::Send(GenericSendError::RxDisconnected))?;
            return Ok(());
        }
        Err(GenericRoutingError::UnknownTargetId(target_id))
    }
}

impl PusRequestRouter<ModeRequest> for GenericRequestRouter {
    type Error = GenericRoutingError;

    fn route(
        &self,
        requestor_info: MessageMetadata,
        target_id: ComponentId,
        request: ModeRequest,
    ) -> Result<(), Self::Error> {
        if let Some(sender) = self.mode_router_map.get(&target_id) {
            sender
                .send(GenericMessage::new(requestor_info, request))
                .map_err(|_| GenericRoutingError::Send(GenericSendError::RxDisconnected))?;
            return Ok(());
        }
        Err(GenericRoutingError::UnknownTargetId(target_id))
    }
}
*/
