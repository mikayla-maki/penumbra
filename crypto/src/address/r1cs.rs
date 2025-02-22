use crate::Address;
use ark_ff::PrimeField;
use ark_r1cs_std::prelude::*;
use ark_relations::r1cs::SynthesisError;
use decaf377::{
    r1cs::{ElementVar, FqVar},
    Element, Fq,
};

pub struct AddressVar {
    pub diversified_generator: ElementVar,
    pub transmission_key: ElementVar,
    pub clue_key: FqVar,
}

impl AddressVar {
    pub fn diversified_generator(&self) -> ElementVar {
        self.diversified_generator.clone()
    }

    pub fn transmission_key(&self) -> ElementVar {
        self.transmission_key.clone()
    }

    pub fn clue_key(&self) -> FqVar {
        self.clue_key.clone()
    }
}

impl AllocVar<Address, Fq> for AddressVar {
    fn new_variable<T: std::borrow::Borrow<Address>>(
        cs: impl Into<ark_relations::r1cs::Namespace<Fq>>,
        f: impl FnOnce() -> Result<T, SynthesisError>,
        mode: ark_r1cs_std::prelude::AllocationMode,
    ) -> Result<Self, SynthesisError> {
        let ns = cs.into();
        let cs = ns.cs();
        let address: Address = *f()?.borrow();
        match mode {
            AllocationMode::Constant => unimplemented!(),
            AllocationMode::Input => unimplemented!(),
            AllocationMode::Witness => {
                let diversified_generator: ElementVar =
                    AllocVar::<Element, Fq>::new_witness(cs.clone(), || {
                        Ok(address.diversified_generator().clone())
                    })?;
                let element_transmission_key = decaf377::Encoding(address.transmission_key().0)
                    .vartime_decompress()
                    .map_err(|_| SynthesisError::AssignmentMissing)?;
                let transmission_key: ElementVar =
                    AllocVar::<Element, Fq>::new_witness(cs.clone(), || {
                        Ok(element_transmission_key)
                    })?;
                let clue_key = FqVar::new_witness(cs, || {
                    Ok(Fq::from_le_bytes_mod_order(&address.clue_key().0[..]))
                })?;

                Ok(Self {
                    diversified_generator,
                    transmission_key,
                    clue_key,
                })
            }
        }
    }
}

// We do not implement the R1CSVar trait for AddressVar since we do not have the
// diversifier in-circuit in order to construct an Address.
