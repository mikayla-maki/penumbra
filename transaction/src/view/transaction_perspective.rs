use penumbra_crypto::{note, AddressView, Note, Nullifier, PayloadKey};
use penumbra_proto::core::transaction::v1alpha1::{
    self as pb, NullifierWithNote, PayloadKeyWithCommitment,
};
use std::collections::BTreeMap;

/// This represents the data to understand an individual transaction without
/// disclosing viewing keys.
pub struct TransactionPerspective {
    /// List of per-action payload keys. These can be used to decrypt
    /// the notes, swaps, and memo keys in the transaction.
    ///
    /// One-to-one correspondence between:
    /// * Output and note,
    /// * Swap and note (NFT),
    ///
    /// There is not a one-to-one correspondence between SwapClaim and notes,
    /// i.e. there are two notes per SwapClaim.
    ///
    /// For outputs, we can use the PayloadKey associated with that output
    /// to decrypt the wrapped_memo_key, which will be used to decrypt the
    /// memo in the transaction. This needs to be done only once, because
    /// there is one memo shared between all outputs.
    pub payload_keys: BTreeMap<note::Commitment, PayloadKey>,
    /// Mapping of nullifiers spent in this transaction to notes.
    pub spend_nullifiers: BTreeMap<Nullifier, Note>,
    /// The openings of note commitments referred to in the transaction but otherwise not included in the transaction.
    pub advice_notes: BTreeMap<note::Commitment, Note>,
    /// The views of any relevant address.
    pub address_views: Vec<AddressView>,
}

impl TransactionPerspective {}

impl From<TransactionPerspective> for pb::TransactionPerspective {
    fn from(msg: TransactionPerspective) -> Self {
        let mut payload_keys = Vec::new();
        let mut spend_nullifiers = Vec::new();
        let mut advice_notes = Vec::new();
        let mut address_views = Vec::new();

        for (commitment, payload_key) in msg.payload_keys {
            payload_keys.push(PayloadKeyWithCommitment {
                payload_key: payload_key.to_vec().into(),
                commitment: Some(commitment.to_owned().into()),
            });
        }

        for (nullifier, note) in msg.spend_nullifiers {
            spend_nullifiers.push(NullifierWithNote {
                nullifier: Some(nullifier.into()),
                note: Some(note.into()),
            })
        }
        for note in msg.advice_notes.into_values() {
            advice_notes.push(note.into());
        }
        for address_view in msg.address_views {
            address_views.push(address_view.into());
        }
        Self {
            payload_keys,
            spend_nullifiers,
            advice_notes,
            address_views,
        }
    }
}

impl TryFrom<pb::TransactionPerspective> for TransactionPerspective {
    type Error = anyhow::Error;

    fn try_from(msg: pb::TransactionPerspective) -> Result<Self, Self::Error> {
        let mut payload_keys = BTreeMap::new();
        let mut spend_nullifiers = BTreeMap::new();
        let mut advice_notes = BTreeMap::new();
        let mut address_views = Vec::new();

        for pk in msg.payload_keys {
            if pk.commitment.is_some() {
                payload_keys.insert(
                    pk.commitment.unwrap().try_into()?,
                    <[u8; 32]>::try_from(pk.payload_key.as_ref())?.into(),
                );
            };
        }

        for nwn in msg.spend_nullifiers {
            spend_nullifiers.insert(
                nwn.nullifier.unwrap().try_into()?,
                nwn.note.unwrap().try_into()?,
            );
        }

        for note in msg.advice_notes {
            let note: Note = note.try_into()?;
            advice_notes.insert(note.commit(), note);
        }

        for address_view in msg.address_views {
            address_views.push(address_view.try_into()?);
        }

        Ok(Self {
            payload_keys,
            spend_nullifiers,
            advice_notes,
            address_views,
        })
    }
}
