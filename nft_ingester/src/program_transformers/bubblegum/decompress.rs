use crate::{
    error::IngesterError,
    program_transformers::bubblegum::{
        upsert_asset_with_compression_info, upsert_asset_with_leaf_info_for_decompression,
    },
};
use blockbuster::{instruction::InstructionBundle, programs::bubblegum::BubblegumInstruction};
use sea_orm::{query::*, ConnectionTrait};

pub async fn decompress<'c, T>(
    _parsing_result: &BubblegumInstruction,
    bundle: &InstructionBundle<'c>,
    txn: &'c T,
) -> Result<(), IngesterError>
where
    T: ConnectionTrait + TransactionTrait,
{
    let id_bytes = bundle.keys.get(3).unwrap().0.as_slice();

    // Begin a transaction.  If the transaction goes out of scope (i.e. one of the executions has
    // an error and this function returns it using the `?` operator), then the transaction is
    // automatically rolled back.
    let multi_txn = txn.begin().await?;

    // Partial update of asset table with just leaf.
    upsert_asset_with_leaf_info_for_decompression(&multi_txn, id_bytes.to_vec()).await?;

    upsert_asset_with_compression_info(
        &multi_txn,
        id_bytes.to_vec(),
        false,
        false,
        1,
        Some(id_bytes.to_vec()),
        true,
    )
    .await?;

    // Commit transaction and relinqish the lock.
    multi_txn.commit().await?;

    Ok(())
}
