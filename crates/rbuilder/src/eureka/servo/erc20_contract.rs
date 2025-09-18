use alloy_primitives::{Address, U256};
use alloy_provider::Provider;
use alloy_sol_types::sol;

sol! {
    #[sol(rpc)]
    interface IERC20 {
        function balanceOf(address owner) external view returns (uint256);
        function allowance(address owner, address spender) external view returns (uint256);
        function approve(address spender, uint256 value) external returns (bool);
        function decimals() external view returns (uint8);
    }
}

#[derive(Clone)]
pub struct Erc20Contract<P: Provider + Clone + Send + Sync + 'static> {
    pub address: Address,
    provider: P,
}

impl<P> Erc20Contract<P>
where
    P: Provider + Clone + Send + Sync + 'static,
{
    pub fn new(address: Address, provider: P) -> Self {
        Self { address, provider }
    }

    pub async fn balance_of(&self, owner: Address) -> eyre::Result<U256> {
        let instance = IERC20::new(self.address, self.provider.clone());
        Ok(instance.balanceOf(owner).call().await?)
    }

    pub async fn allowance(&self, owner: Address, spender: Address) -> eyre::Result<U256> {
        let instance = IERC20::new(self.address, self.provider.clone());
        Ok(instance.allowance(owner, spender).call().await?)
    }

    // Note: approve is a state-changing tx. For now, only balance/allowance are exposed. TODO: add approve tx sending if needed.
}
