pub mod common {
    pub mod v1 {
        include!("gen/plfm.common.v1.rs");
    }
}

pub mod events {
    pub mod v1 {
        include!("gen/plfm.events.v1.rs");
    }
}

pub mod agent {
    pub mod v1 {
        include!("gen/plfm.agent.v1.rs");

        pub use node_agent_client::NodeAgentClient;
        pub use node_agent_server::{NodeAgent, NodeAgentServer};
    }
}
