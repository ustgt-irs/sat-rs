#[derive(
    serde::Serialize,
    serde::Deserialize,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    num_enum::TryFromPrimitive,
    num_enum::IntoPrimitive,
)]
#[repr(u32)]
pub enum Mode {
    Off = 0,
    Safe = 1,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeChild {
    MgmAssembly,
    Mgt,
    Controller,
}

impl Mode {
    /// Each subsystem mode has a fallback mode to allow clean transitions.
    pub const fn fallback_mode(&self) -> Self {
        match self {
            Mode::Off => Mode::Safe,
            Mode::Safe => Mode::Safe,
        }
    }
}

pub mod request {
    use crate::Message;

    #[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub enum ModeRequest {
        SetMode(super::Mode),
        ReadMode,
    }

    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
    pub enum Request {
        Ping,
        Mode(ModeRequest),
    }

    impl Request {
        fn message_type(&self) -> crate::MessageType {
            match self {
                Request::Ping => crate::MessageType::Verification,
                Request::Mode(_mode) => crate::MessageType::Mode,
            }
        }
    }

    impl Message for Request {
        fn message_type(&self) -> crate::MessageType {
            self.message_type()
        }
    }
}

pub mod response {
    use crate::Message;

    #[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub enum ModeResponse {
        /// Mode of the subsystem.
        Mode(super::Mode),
        /// Command timeout when commanding a child.
        CommandTimeout(super::ModeChild),
        /// Can not keep mode because a child changed mode unexpectedly.
        CanNotKeepMode(super::ModeChild),
    }

    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
    pub enum Response {
        Ok,
        Mode(ModeResponse),
    }

    impl Response {
        fn message_type(&self) -> crate::MessageType {
            match self {
                Response::Ok => crate::MessageType::Verification,
                Response::Mode(_mode_report) => crate::MessageType::Mode,
            }
        }
    }

    impl Message for Response {
        fn message_type(&self) -> crate::MessageType {
            self.message_type()
        }
    }
}
