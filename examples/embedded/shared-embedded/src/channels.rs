//! All queues between the tasks. Senders and receivers borrow their queue, so the application keeps
//! the queues in a static.

use crate::event_manager::{EventChannel, EventManagerEndpoints};
use crate::net::UdpEndpoints;
use crate::pcdu::{PcduEndpoints, PcduSwitchRequestChannel, PcduTcChannel};
use crate::sim_client::{
    PcduSimReplyChannel, SimClientEndpoints, SimReplySenders, SimRequestChannel,
};
use crate::tmtc::{
    TcChannel, TcHandlerEndpoints, TcSenders, TmChannel, TmFunnelChannel, TmFunnelEndpoints,
};

pub struct Channels {
    tc: TcChannel,
    tm: TmChannel,
    tm_funnel: TmFunnelChannel,
    event: EventChannel,
    sim_request: SimRequestChannel,
    pcdu_sim_reply: PcduSimReplyChannel,
    pcdu_tc: PcduTcChannel,
    /// No component sends switch requests yet.
    pcdu_switch_request: PcduSwitchRequestChannel,
}

impl Default for Channels {
    fn default() -> Self {
        Self::new()
    }
}

impl Channels {
    pub const fn new() -> Self {
        Self {
            tc: TcChannel::new(),
            tm: TmChannel::new(),
            tm_funnel: TmFunnelChannel::new(),
            event: EventChannel::new(),
            sim_request: SimRequestChannel::new(),
            pcdu_sim_reply: PcduSimReplyChannel::new(),
            pcdu_tc: PcduTcChannel::new(),
            pcdu_switch_request: PcduSwitchRequestChannel::new(),
        }
    }

    pub fn udp(&'static self) -> UdpEndpoints {
        UdpEndpoints {
            tc_tx: self.tc.sender(),
            tm_rx: self.tm.receiver(),
        }
    }

    pub fn sim_client(&'static self) -> SimClientEndpoints {
        SimClientEndpoints {
            request_rx: self.sim_request.receiver(),
            reply_senders: SimReplySenders {
                pcdu: self.pcdu_sim_reply.sender(),
            },
        }
    }

    pub fn tc_handler(&'static self) -> TcHandlerEndpoints {
        TcHandlerEndpoints {
            tc_rx: self.tc.receiver(),
            tm_tx: self.tm_funnel.sender(),
            event_tx: self.event.sender(),
            tc_senders: TcSenders {
                pcdu: self.pcdu_tc.sender(),
            },
        }
    }

    pub fn tm_funnel(&'static self) -> TmFunnelEndpoints {
        TmFunnelEndpoints {
            funnel_rx: self.tm_funnel.receiver(),
            tm_tx: self.tm.sender(),
        }
    }

    pub fn event_manager(&'static self) -> EventManagerEndpoints {
        EventManagerEndpoints {
            event_rx: self.event.receiver(),
            tm_tx: self.tm_funnel.sender(),
        }
    }

    pub fn pcdu(&'static self) -> PcduEndpoints {
        PcduEndpoints {
            tc_rx: self.pcdu_tc.receiver(),
            tm_tx: self.tm_funnel.sender(),
            switch_request_rx: self.pcdu_switch_request.receiver(),
            event_tx: self.event.sender(),
            sim_request_tx: self.sim_request.sender(),
            sim_reply_rx: self.pcdu_sim_reply.receiver(),
        }
    }
}
