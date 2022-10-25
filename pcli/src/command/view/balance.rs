use std::collections::BTreeMap;

use anyhow::Result;
use comfy_table::{presets, Table};
use penumbra_crypto::{keys::AddressIndex, Amount, FullViewingKey, Value};
use penumbra_view::{QuarantinedNoteRecord, SpendableNoteRecord, ViewClient};
#[derive(Debug, clap::Args)]
pub struct BalanceCmd {
    /// If set, breaks down balances by address.
    #[clap(short, long)]
    pub by_address: bool,
    #[clap(long)]
    /// If set, prints the value of each note individually.
    pub by_note: bool,
}

impl BalanceCmd {
    pub fn offline(&self) -> bool {
        false
    }

    pub async fn exec<V: ViewClient>(&self, fvk: &FullViewingKey, view: &mut V) -> Result<()> {
        let asset_cache = view.assets().await?;

        // Initialize the table
        let mut table = Table::new();
        table.load_preset(presets::NOTHING);

        // `Option<u64>` indicates the unbonding epoch, if any, for a quarantined note
        let rows: Vec<(Option<AddressIndex>, Value, Option<u64>)> = if self.by_address {
            let notes = view.unspent_notes_by_address_and_asset(fvk.hash()).await?;
            let quarantined_notes = view
                .quarantined_notes_by_address_and_asset(fvk.hash())
                .await?;

            if self.by_note {
                collect_by_note(
                    notes,
                    quarantined_notes,
                    |index, _| Some(*index),
                    |_, asset, amount| asset.value(amount),
                )
            } else {
                sum_by_asset(
                    notes,
                    quarantined_notes,
                    |index, _| Some(*index),
                    |_, asset, amount| asset.value(amount),
                )
            }
        } else {
            let notes = view.unspent_notes_by_asset_and_address(fvk.hash()).await?;
            let quarantined_notes = view
                .quarantined_notes_by_asset_and_address(fvk.hash())
                .await?;

            if self.by_note {
                collect_by_note(
                    notes,
                    quarantined_notes,
                    |_, index| Some(*index),
                    |asset, _, amount| asset.value(amount),
                )
            } else {
                sum_by_asset(
                    notes,
                    quarantined_notes,
                    |_, _| None,
                    |asset, _, amount| asset.value(amount),
                )
            }
        };

        if self.by_address {
            table.set_header(vec!["Addr Index", "Amount"]);
        } else {
            table.set_header(vec!["Amount"]);
        }

        for (index, value, quarantined) in rows {
            let mut row = Vec::with_capacity(2);

            let index =
                u128::from(index.expect("--by-address specified, but no address set for note"));
            let index_text = if index < u64::MAX as u128 {
                format!("{}", index)
            } else {
                "Eph".to_string()
            };

            if self.by_address {
                row.push(index_text)
            }

            row.push(format!(
                "{}{}",
                value.format(&asset_cache),
                if let Some(unbonding_epoch) = quarantined {
                    format!(" (unbonding until epoch {})", unbonding_epoch)
                } else {
                    "".to_string()
                }
            ));

            table.add_row(row);
        }

        println!("{}", table);

        Ok(())
    }
}

// G1 / G2 mean 'group 1' and 'group 2'. Depending on the code path, these
// should be either `AddressIndex, Id` or `Id, AddressIndex`. Due to the
// indeterminate order, we have to use generics and we have to pass in the selector
// functions to disambiguate the two
fn collect_by_note<G1, G2, F1, F2>(
    notes: BTreeMap<G1, BTreeMap<G2, Vec<SpendableNoteRecord>>>,
    quarantined_notes: BTreeMap<G1, BTreeMap<G2, Vec<QuarantinedNoteRecord>>>,
    index_selector: F1,
    value_selector: F2,
) -> Vec<(Option<AddressIndex>, Value, Option<u64>)>
where
    F1: Fn(&G1, &G2) -> Option<AddressIndex>,
    F2: Fn(&G1, &G2, Amount) -> Value,
{
    notes
        .iter()
        .flat_map(|(g1, notes_by_g1)| {
            // Include each note individually:
            notes_by_g1.iter().flat_map(|(g2, notes_by_g2)| {
                notes_by_g2.iter().map(|record| {
                    (
                        index_selector(g1, g2),
                        value_selector(g1, g2, record.note.amount()),
                        None,
                    )
                })
            })
        })
        .chain(quarantined_notes.iter().flat_map(|(g1, notes_by_g1)| {
            // Include each note individually:
            notes_by_g1.iter().flat_map(|(g2, notes_by_g2)| {
                notes_by_g2.iter().map(|record| {
                    (
                        index_selector(g1, g2),
                        value_selector(g1, g2, record.note.amount()),
                        Some(record.unbonding_epoch),
                    )
                })
            })
        }))
        .collect()
}

// G1 / G2 mean 'group 1' and 'group 2'. Depending on the code path, these
// should be either `AddressIndex, Id` or `Id, AddressIndex`. Due to the
// indeterminate order, we have to use generics and we have to pass in the selector
// functions to disambiguate the two
fn sum_by_asset<G1, G2, F1, F2>(
    notes: BTreeMap<G1, BTreeMap<G2, Vec<SpendableNoteRecord>>>,
    quarantined_notes: BTreeMap<G1, BTreeMap<G2, Vec<QuarantinedNoteRecord>>>,
    index_selector: F1,
    value_selector: F2,
) -> Vec<(Option<AddressIndex>, Value, Option<u64>)>
where
    F1: Fn(&G1, &G2) -> Option<AddressIndex>,
    F2: Fn(&G1, &G2, Amount) -> Value,
{
    notes
        .iter()
        .flat_map(|(g1, notes_by_g1)| {
            // Sum the notes for each asset:
            notes_by_g1.iter().map(|(g2, notes_by_g2)| {
                let sum: u64 = notes_by_g2
                    .iter()
                    .map(|record| u64::from(record.note.amount()))
                    .sum();
                (
                    index_selector(g1, g2),
                    value_selector(g1, g2, sum.into()),
                    None,
                )
            })
        })
        .chain(quarantined_notes.iter().flat_map(|(g1, notes_by_g1)| {
            // Sum the notes for each asset, separating them by unbonding epoch:
            notes_by_g1.iter().flat_map(|(g2, notes_by_g2)| {
                let mut sums_by_unbonding_epoch = BTreeMap::<u64, u64>::new();
                for record in notes_by_g2 {
                    let unbonding_epoch = record.unbonding_epoch;
                    *sums_by_unbonding_epoch.entry(unbonding_epoch).or_default() +=
                        u64::from(record.note.amount());
                }
                sums_by_unbonding_epoch
                    .into_iter()
                    .map(|(unbonding_epoch, sum)| {
                        (
                            index_selector(g1, g2),
                            value_selector(g1, g2, sum.into()),
                            Some(unbonding_epoch),
                        )
                    })
            })
        }))
        .collect()
}
