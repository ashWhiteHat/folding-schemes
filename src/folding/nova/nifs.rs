use ark_crypto_primitives::sponge::Absorb;
use ark_ec::{CurveGroup, Group};
use ark_std::One;
use std::marker::PhantomData;

use crate::ccs::r1cs::R1CS;
use crate::folding::nova::{CommittedInstance, Witness};
use crate::pedersen::{Params as PedersenParams, Pedersen, Proof as PedersenProof};
use crate::transcript::Transcript;
use crate::utils::vec::*;
use crate::Error;

/// Implements the Non-Interactive Folding Scheme described in section 4 of
/// https://eprint.iacr.org/2021/370.pdf
pub struct NIFS<C: CurveGroup> {
    _phantom: PhantomData<C>,
}

impl<C: CurveGroup> NIFS<C>
where
    <C as Group>::ScalarField: Absorb,
{
    // compute_T: compute cross-terms T
    pub fn compute_T(
        r1cs: &R1CS<C::ScalarField>,
        u1: C::ScalarField,
        u2: C::ScalarField,
        z1: &[C::ScalarField],
        z2: &[C::ScalarField],
    ) -> Result<Vec<C::ScalarField>, Error> {
        let (A, B, C) = (r1cs.A.clone(), r1cs.B.clone(), r1cs.C.clone());

        // this is parallelizable (for the future)
        let Az1 = mat_vec_mul_sparse(&A, z1);
        let Bz1 = mat_vec_mul_sparse(&B, z1);
        let Cz1 = mat_vec_mul_sparse(&C, z1);
        let Az2 = mat_vec_mul_sparse(&A, z2);
        let Bz2 = mat_vec_mul_sparse(&B, z2);
        let Cz2 = mat_vec_mul_sparse(&C, z2);

        let Az1_Bz2 = hadamard(&Az1, &Bz2)?;
        let Az2_Bz1 = hadamard(&Az2, &Bz1)?;
        let u1Cz2 = vec_scalar_mul(&Cz2, &u1);
        let u2Cz1 = vec_scalar_mul(&Cz1, &u2);

        vec_sub(&vec_sub(&vec_add(&Az1_Bz2, &Az2_Bz1)?, &u1Cz2)?, &u2Cz1)
    }

    pub fn fold_witness(
        r: C::ScalarField,
        w1: &Witness<C>,
        w2: &Witness<C>,
        T: &[C::ScalarField],
        rT: C::ScalarField,
    ) -> Result<Witness<C>, Error> {
        let r2 = r * r;
        let E: Vec<C::ScalarField> = vec_add(
            &vec_add(&w1.E, &vec_scalar_mul(T, &r))?,
            &vec_scalar_mul(&w2.E, &r2),
        )?;
        let rE = w1.rE + r * rT + r2 * w2.rE;
        let W: Vec<C::ScalarField> = w1.W.iter().zip(&w2.W).map(|(a, b)| *a + (r * b)).collect();

        let rW = w1.rW + r * w2.rW;
        Ok(Witness::<C> { E, rE, W, rW })
    }

    pub fn fold_committed_instance(
        r: C::ScalarField,
        ci1: &CommittedInstance<C>,
        ci2: &CommittedInstance<C>,
        cmT: &C,
    ) -> CommittedInstance<C> {
        let r2 = r * r;
        let cmE = ci1.cmE + cmT.mul(r) + ci2.cmE.mul(r2);
        let u = ci1.u + r * ci2.u;
        let cmW = ci1.cmW + ci2.cmW.mul(r);
        let x = ci1
            .x
            .iter()
            .zip(&ci2.x)
            .map(|(a, b)| *a + (r * b))
            .collect::<Vec<C::ScalarField>>();

        CommittedInstance::<C> { cmE, u, cmW, x }
    }

    // NIFS.P
    #[allow(clippy::type_complexity)]
    pub fn prove(
        pedersen_params: &PedersenParams<C>,
        // r comes from the transcript, and is a n-bit (N_BITS_CHALLENGE) element
        r: C::ScalarField,
        r1cs: &R1CS<C::ScalarField>,
        w1: &Witness<C>,
        ci1: &CommittedInstance<C>,
        w2: &Witness<C>,
        ci2: &CommittedInstance<C>,
    ) -> Result<(Witness<C>, CommittedInstance<C>, Vec<C::ScalarField>, C), Error> {
        let z1: Vec<C::ScalarField> = [vec![ci1.u], ci1.x.to_vec(), w1.W.to_vec()].concat();
        let z2: Vec<C::ScalarField> = [vec![ci2.u], ci2.x.to_vec(), w2.W.to_vec()].concat();

        // compute cross terms
        let T = Self::compute_T(r1cs, ci1.u, ci2.u, &z1, &z2)?;
        let rT = C::ScalarField::one(); // use 1 as rT since we don't need hiding property for cm(T)
        let cmT = Pedersen::commit(pedersen_params, &T, &rT)?;

        // fold witness
        let w3 = NIFS::<C>::fold_witness(r, w1, w2, &T, rT)?;

        // fold committed instancs
        let ci3 = NIFS::<C>::fold_committed_instance(r, ci1, ci2, &cmT);

        Ok((w3, ci3, T, cmT))
    }

    // NIFS.V
    pub fn verify(
        // r comes from the transcript, and is a n-bit (N_BITS_CHALLENGE) element
        r: C::ScalarField,
        ci1: &CommittedInstance<C>,
        ci2: &CommittedInstance<C>,
        cmT: &C,
    ) -> CommittedInstance<C> {
        NIFS::<C>::fold_committed_instance(r, ci1, ci2, cmT)
    }

    /// Verify commited folded instance (ci) relations. Notice that this method does not open the
    /// commitments, but just checks that the given committed instances (ci1, ci2) when folded
    /// result in the folded committed instance (ci3) values.
    pub fn verify_folded_instance(
        r: C::ScalarField,
        ci1: &CommittedInstance<C>,
        ci2: &CommittedInstance<C>,
        ci3: &CommittedInstance<C>,
        cmT: &C,
    ) -> Result<(), Error> {
        let r2 = r * r;
        if ci3.cmE != (ci1.cmE + cmT.mul(r) + ci2.cmE.mul(r2))
            || ci3.u != ci1.u + r * ci2.u
            || ci3.cmW != (ci1.cmW + ci2.cmW.mul(r))
            || ci3.x != vec_add(&ci1.x, &vec_scalar_mul(&ci2.x, &r))?
        {
            return Err(Error::NotSatisfied);
        }
        Ok(())
    }

    pub fn open_commitments(
        tr: &mut impl Transcript<C>,
        pedersen_params: &PedersenParams<C>,
        w: &Witness<C>,
        ci: &CommittedInstance<C>,
        T: Vec<C::ScalarField>,
        cmT: &C,
    ) -> Result<[PedersenProof<C>; 3], Error> {
        let cmE_proof = Pedersen::prove(pedersen_params, tr, &ci.cmE, &w.E, &w.rE)?;
        let cmW_proof = Pedersen::prove(pedersen_params, tr, &ci.cmW, &w.W, &w.rW)?;
        let cmT_proof = Pedersen::prove(pedersen_params, tr, cmT, &T, &C::ScalarField::one())?; // cm(T) is committed with rT=1
        Ok([cmE_proof, cmW_proof, cmT_proof])
    }
    pub fn verify_commitments(
        tr: &mut impl Transcript<C>,
        pedersen_params: &PedersenParams<C>,
        ci: CommittedInstance<C>,
        cmT: C,
        cm_proofs: [PedersenProof<C>; 3],
    ) -> Result<(), Error> {
        if cm_proofs.len() != 3 {
            // cm_proofs should have length 3: [cmE_proof, cmW_proof, cmT_proof]
            return Err(Error::NotExpectedLength);
        }
        Pedersen::verify(pedersen_params, tr, ci.cmE, cm_proofs[0].clone())?;
        Pedersen::verify(pedersen_params, tr, ci.cmW, cm_proofs[1].clone())?;
        Pedersen::verify(pedersen_params, tr, cmT, cm_proofs[2].clone())?;
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use ark_ff::PrimeField;
    use ark_pallas::{Fr, Projective};
    use ark_std::{ops::Mul, UniformRand, Zero};

    use crate::ccs::r1cs::tests::{get_test_r1cs, get_test_z};
    use crate::transcript::poseidon::{tests::poseidon_test_config, PoseidonTranscript};
    use crate::utils::vec::vec_scalar_mul;

    pub fn check_relaxed_r1cs<F: PrimeField>(r1cs: &R1CS<F>, z: &[F], u: &F, E: &[F]) -> bool {
        let Az = mat_vec_mul_sparse(&r1cs.A, z);
        let Bz = mat_vec_mul_sparse(&r1cs.B, z);
        let Cz = mat_vec_mul_sparse(&r1cs.C, z);
        hadamard(&Az, &Bz).unwrap() == vec_add(&vec_scalar_mul(&Cz, u), E).unwrap()
    }

    // fold 2 dummy instances and check that the folded instance holds the relaxed R1CS relation
    #[test]
    fn test_nifs_fold_dummy() {
        let r1cs = get_test_r1cs::<Fr>();
        let z1 = get_test_z(3);
        let (w1, x1) = r1cs.split_z(&z1);

        let mut rng = ark_std::test_rng();
        let pedersen_params = Pedersen::<Projective>::new_params(&mut rng, r1cs.A.n_cols);

        // dummy instance, witness and public inputs zeroes
        let w_dummy = Witness::<Projective>::new(vec![Fr::zero(); w1.len()], r1cs.A.n_rows);
        let mut u_dummy = w_dummy
            .commit(&pedersen_params, vec![Fr::zero(); x1.len()])
            .unwrap();
        u_dummy.u = Fr::zero();

        let w_i = w_dummy.clone();
        let u_i = u_dummy.clone();
        let W_i = w_dummy.clone();
        let U_i = u_dummy.clone();
        let z_dummy = vec![Fr::zero(); z1.len()];
        assert!(check_relaxed_r1cs(&r1cs, &z_dummy, &u_i.u, &w_i.E));
        assert!(check_relaxed_r1cs(&r1cs, &z_dummy, &U_i.u, &W_i.E));

        let r_Fr = Fr::from(3_u32);

        let (W_i1, U_i1, _, _) =
            NIFS::<Projective>::prove(&pedersen_params, r_Fr, &r1cs, &w_i, &u_i, &W_i, &U_i)
                .unwrap();

        let z: Vec<Fr> = [vec![U_i1.u], U_i1.x.to_vec(), W_i1.W.to_vec()].concat();
        assert_eq!(z.len(), z1.len());
        assert!(check_relaxed_r1cs(&r1cs, &z, &U_i1.u, &W_i1.E));
    }

    // fold 2 instances into one
    #[test]
    fn test_nifs_one_fold() {
        let r1cs = get_test_r1cs();
        let z1 = get_test_z(3);
        let z2 = get_test_z(4);
        let (w1, x1) = r1cs.split_z(&z1);
        let (w2, x2) = r1cs.split_z(&z2);

        let w1 = Witness::<Projective>::new(w1.clone(), r1cs.A.n_rows);
        let w2 = Witness::<Projective>::new(w2.clone(), r1cs.A.n_rows);

        let mut rng = ark_std::test_rng();
        let pedersen_params = Pedersen::<Projective>::new_params(&mut rng, r1cs.A.n_cols);

        let r = Fr::rand(&mut rng); // folding challenge would come from the transcript

        // compute committed instances
        let ci1 = w1.commit(&pedersen_params, x1.clone()).unwrap();
        let ci2 = w2.commit(&pedersen_params, x2.clone()).unwrap();

        // NIFS.P
        let (w3, ci3_aux, T, cmT) =
            NIFS::<Projective>::prove(&pedersen_params, r, &r1cs, &w1, &ci1, &w2, &ci2).unwrap();

        // NIFS.V
        let ci3 = NIFS::<Projective>::verify(r, &ci1, &ci2, &cmT);
        assert_eq!(ci3, ci3_aux);

        // naive check that the folded witness satisfies the relaxed r1cs
        let z3: Vec<Fr> = [vec![ci3.u], ci3.x.to_vec(), w3.W.to_vec()].concat();
        // check that z3 is as expected
        let z3_aux = vec_add(&z1, &vec_scalar_mul(&z2, &r)).unwrap();
        assert_eq!(z3, z3_aux);
        // check that relations hold for the 2 inputted instances and the folded one
        assert!(check_relaxed_r1cs(&r1cs, &z1, &ci1.u, &w1.E));
        assert!(check_relaxed_r1cs(&r1cs, &z2, &ci2.u, &w2.E));
        assert!(check_relaxed_r1cs(&r1cs, &z3, &ci3.u, &w3.E));

        // check that folded commitments from folded instance (ci) are equal to folding the
        // use folded rE, rW to commit w3
        let ci3_expected = w3.commit(&pedersen_params, ci3.x.clone()).unwrap();
        assert_eq!(ci3_expected.cmE, ci3.cmE);
        assert_eq!(ci3_expected.cmW, ci3.cmW);

        // next equalities should hold since we started from two cmE of zero-vector E's
        assert_eq!(ci3.cmE, cmT.mul(r));
        assert_eq!(w3.E, vec_scalar_mul(&T, &r));

        // NIFS.Verify_Folded_Instance:
        NIFS::<Projective>::verify_folded_instance(r, &ci1, &ci2, &ci3, &cmT).unwrap();

        let poseidon_config = poseidon_test_config::<Fr>();
        // init Prover's transcript
        let mut transcript_p = PoseidonTranscript::<Projective>::new(&poseidon_config);
        // init Verifier's transcript
        let mut transcript_v = PoseidonTranscript::<Projective>::new(&poseidon_config);

        // check openings of ci3.cmE, ci3.cmW and cmT
        let cm_proofs = NIFS::<Projective>::open_commitments(
            &mut transcript_p,
            &pedersen_params,
            &w3,
            &ci3,
            T,
            &cmT,
        )
        .unwrap();

        NIFS::<Projective>::verify_commitments(
            &mut transcript_v,
            &pedersen_params,
            ci3,
            cmT,
            cm_proofs,
        )
        .unwrap();
    }

    #[test]
    fn test_nifs_fold_loop() {
        let r1cs = get_test_r1cs();
        let z = get_test_z(3);
        let (w, x) = r1cs.split_z(&z);

        let mut rng = ark_std::test_rng();
        let pedersen_params = Pedersen::<Projective>::new_params(&mut rng, r1cs.A.n_cols);

        // prepare the running instance
        let mut running_instance_w = Witness::<Projective>::new(w.clone(), r1cs.A.n_rows);
        let mut running_committed_instance =
            running_instance_w.commit(&pedersen_params, x).unwrap();
        assert!(check_relaxed_r1cs(
            &r1cs,
            &z,
            &running_committed_instance.u,
            &running_instance_w.E,
        ));

        let num_iters = 10;
        for i in 0..num_iters {
            // prepare the incomming instance
            let incomming_instance_z = get_test_z(i + 4);
            let (w, x) = r1cs.split_z(&incomming_instance_z);
            let incomming_instance_w = Witness::<Projective>::new(w.clone(), r1cs.A.n_rows);
            let incomming_committed_instance =
                incomming_instance_w.commit(&pedersen_params, x).unwrap();
            assert!(check_relaxed_r1cs(
                &r1cs,
                &incomming_instance_z.clone(),
                &incomming_committed_instance.u,
                &incomming_instance_w.E,
            ));

            let r = Fr::rand(&mut rng); // folding challenge would come from the transcript

            // NIFS.P
            let (folded_w, _, _, cmT) = NIFS::<Projective>::prove(
                &pedersen_params,
                r,
                &r1cs,
                &running_instance_w,
                &running_committed_instance,
                &incomming_instance_w,
                &incomming_committed_instance,
            )
            .unwrap();

            // NIFS.V
            let folded_committed_instance = NIFS::<Projective>::verify(
                r,
                &running_committed_instance,
                &incomming_committed_instance,
                &cmT,
            );

            let folded_z: Vec<Fr> = [
                vec![folded_committed_instance.u],
                folded_committed_instance.x.to_vec(),
                folded_w.W.to_vec(),
            ]
            .concat();

            assert!(check_relaxed_r1cs(
                &r1cs,
                &folded_z,
                &folded_committed_instance.u,
                &folded_w.E
            ));

            // set running_instance for next loop iteration
            running_instance_w = folded_w;
            running_committed_instance = folded_committed_instance;
        }
    }
}
