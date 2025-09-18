use alloy_primitives::{Address, B256, U256};
use alloy_provider::Provider;
use alloy_sol_types::sol;

// Minimal Servo stake contract interface based on ebuilder usage.
sol! {
    #[sol(rpc)]
    interface ServoStake {
        function stakeToken() external view returns (address);
        function stakedAmount(address staker) external view returns (uint256);
        function getStakeCommitmentHash(
            address staker,
            uint256 stakeChannelNonce,
            uint256 stakeCommitmentNonce,
            bytes32 previousCommitmentHash,
            uint256 stakeSpentAmount
        ) external view returns (bytes32);
    }
}

#[derive(Clone)]
pub struct ServoStakeContract<P: Provider + Clone + Send + Sync + 'static> {
    pub address: Address,
    provider: P,
}

impl<P> ServoStakeContract<P>
where
    P: Provider + Clone + Send + Sync + 'static,
{
    pub fn new(address: Address, provider: P) -> Self {
        Self { address, provider }
    }

    /// Validate deployment by checking stakeToken() call succeeds and returns non-zero address.
    pub async fn validate_deployment(&self) -> eyre::Result<Address> {
        let instance = ServoStake::new(self.address, self.provider.clone());
        let token_fb = instance.stakeToken().call().await?.0;
        let token = Address::from_slice(token_fb.as_slice());
        if token.is_zero() {
            eyre::bail!("stakeToken() returned zero address");
        }
        Ok(token)
    }

    pub async fn get_staked_amount(&self, staker: Address) -> eyre::Result<U256> {
        let instance = ServoStake::new(self.address, self.provider.clone());
        Ok(instance.stakedAmount(staker).call().await?)
    }

    pub async fn get_stake_commitment_hash(
        &self,
        staker: Address,
        stake_channel_nonce: U256,
        stake_commitment_nonce: U256,
        previous_commitment_hash: B256,
        stake_spent_amount: U256,
    ) -> eyre::Result<B256> {
        let instance = ServoStake::new(self.address, self.provider.clone());
        let ret: B256 = instance
            .getStakeCommitmentHash(
                staker,
                stake_channel_nonce,
                stake_commitment_nonce,
                previous_commitment_hash,
                stake_spent_amount,
            )
            .call()
            .await?;
        Ok(ret)
    }
}
