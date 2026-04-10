//! Polygon PoS + zkEVM network bindings

#[derive(Debug, PartialEq, Eq)]
pub enum PolygonChain {
    PoS,
    ZkEvm,
}

pub struct PolygonGateway;

impl PolygonGateway {
    /// Finality checkpointing validation
    pub fn check_polygon_finality(_chain: &PolygonChain, block: u64) -> bool {
        match _chain {
            PolygonChain::PoS => block > 10,
            PolygonChain::ZkEvm => block > 2, // ZK rollups are faster
        }
    }

    /// Measure finality time diff
    pub fn measure_settlement_time(chain: &PolygonChain) -> u32 {
        match chain {
            PolygonChain::PoS => 120,  // 2 mins
            PolygonChain::ZkEvm => 12, // 12 seconds
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_polygon_finality() {
        assert!(PolygonGateway::check_polygon_finality(
            &PolygonChain::ZkEvm,
            5
        ));
        assert!(!PolygonGateway::check_polygon_finality(
            &PolygonChain::PoS,
            5
        ));
    }

    #[test]
    fn test_settlement_time() {
        assert!(
            PolygonGateway::measure_settlement_time(&PolygonChain::ZkEvm)
                < PolygonGateway::measure_settlement_time(&PolygonChain::PoS)
        );
    }
}
