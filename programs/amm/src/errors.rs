use anchor_lang::prelude::*;
use constant_product_curve::CurveError;

#[error_code]
pub enum AmmError {
    #[msg("Amount must be greater than zero")]
    InvalidAmount,
    #[msg("Fee cannot exceed 10000 basis points")]
    InvalidFee,
    #[msg("Both mints must be different")]
    IdenticalMints,
    #[msg("The pool is locked")]
    PoolLocked,
    #[msg("Slippage limit exceeded")]
    SlippageExceeded,
    #[msg("The pool holds no liquidity")]
    NoLiquidity,
    #[msg("There are no fees to claim")]
    NothingToClaim,
    #[msg("Curve arithmetic overflowed")]
    Overflow,
    #[msg("Curve arithmetic underflowed")]
    Underflow,
    #[msg("The curve rejected the operation")]
    CurveFailure,
}

impl From<CurveError> for AmmError {
    fn from(error: CurveError) -> Self {
        match error {
            CurveError::Overflow => AmmError::Overflow,
            CurveError::Underflow => AmmError::Underflow,
            CurveError::SlippageLimitExceeded => AmmError::SlippageExceeded,
            CurveError::ZeroBalance => AmmError::NoLiquidity,
            CurveError::InsufficientBalance => AmmError::NoLiquidity,
            CurveError::InvalidFeeAmount => AmmError::InvalidFee,
            CurveError::InvalidPrecision => AmmError::CurveFailure,
        }
    }
}
