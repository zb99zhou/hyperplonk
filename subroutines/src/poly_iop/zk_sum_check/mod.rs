use crate::poly_iop::{
    errors::PolyIOPErrors,
    structs::{IOPProof, IOPVerifierState},
    PolyIOP,
};
use arithmetic::{VPAuxInfo, VirtualPolynomial};
use ark_ff::PrimeField;
use ark_poly::DenseMultilinearExtension;
use ark_std::{end_timer, start_timer};
use prover::{RandomMaskPolynomial, ZkSumCheckProverState};
use std::{fmt::Debug, sync::Arc};
use transcript::IOPTranscript;

mod prover;
mod verifier;

/// Trait for doing zk sum check protocols.
pub trait ZkSumCheck<F: PrimeField> {
    type VirtualPolynomial;
    type VPAuxInfo;
    type MultilinearExtension;
    type RandomMaskPolynomial;
    type MPNumV;
    type MPDeg;

    type SumCheckProof: Clone + Debug + Default + PartialEq;
    type Transcript;
    type SumCheckSubClaim: Clone + Debug + Default + PartialEq;

    /// Extract sum from the proof
    fn extract_sum(proof: &Self::SumCheckProof) -> F;

    /// Initialize the system with a transcript
    ///
    /// This function is optional -- in the case where a SumCheck is
    /// an building block for a more complex protocol, the transcript
    /// may be initialized by this complex protocol, and passed to the
    /// SumCheck prover/verifier.
    fn init_transcript() -> Self::Transcript;

    /// Generate proof of the sum of polynomial over {0,1}^`num_vars`
    ///
    /// The polynomial is represented in the form of a VirtualPolynomial.
    fn prove(
        poly: &Self::VirtualPolynomial,
        mask_poly: &Self::RandomMaskPolynomial,
        rho: &F,
        transcript: &mut Self::Transcript,
    ) -> Result<Self::SumCheckProof, PolyIOPErrors>;

    /// Verify the claimed sum using the proof
    fn verify(
        sum: F,
        proof: &Self::SumCheckProof,
        aux_info: &Self::VPAuxInfo,
        transcript: &mut Self::Transcript,
        mask_poly_nv: Self::MPNumV,
        mask_poly_degree: Self::MPDeg
    ) -> Result<Self::SumCheckSubClaim, PolyIOPErrors>;
}

/// Trait for zk sum check protocol prover side APIs.
pub trait ZkSumCheckProver<F: PrimeField>
where
    Self: Sized,
{
    type VirtualPolynomial;
    type ProverMessage;
    type RandomMaskPolynomial;

    /// Initialize the prover state to argue for the sum of the input polynomial
    /// over {0,1}^`num_vars`.
    fn prover_init(polynomial: &Self::VirtualPolynomial, mask_poly: &Self::RandomMaskPolynomial) -> Result<Self, PolyIOPErrors>;

    /// Receive message from verifier, generate prover message, and proceed to
    /// next round.
    fn prove_round_and_update_state(
        &mut self,
        rho: &F,
        challenge: &Option<F>,
    ) -> Result<Self::ProverMessage, PolyIOPErrors>;
}

/// Trait for zk sum check protocol verifier side APIs.
pub trait ZkSumCheckVerifier<F: PrimeField> {
    type VPAuxInfo;
    type ProverMessage;
    type Challenge;
    type Transcript;
    type ZkSumCheckSubClaim;
    type MPNumV;
    type MPDeg;

    /// Initialize the verifier's state.
    fn verifier_init(index_info: &Self::VPAuxInfo) -> Self;

    /// Run verifier for the current round, given a prover message.
    ///
    /// Note that `verify_round_and_update_state` only samples and stores
    /// challenges; and update the verifier's state accordingly. The actual
    /// verifications are deferred (in batch) to `check_and_generate_subclaim`
    /// at the last step.
    fn verify_round_and_update_state(
        &mut self,
        prover_msg: &Self::ProverMessage,
        transcript: &mut Self::Transcript,
    ) -> Result<Self::Challenge, PolyIOPErrors>;

    /// This function verifies the deferred checks in the interactive version of
    /// the protocol; and generate the subclaim. Returns an error if the
    /// proof failed to verify.
    ///
    /// If the asserted sum is correct, then the multilinear polynomial
    /// evaluated at `subclaim.point` will be `subclaim.expected_evaluation`.
    /// Otherwise, it is highly unlikely that those two will be equal.
    /// Larger field size guarantees smaller soundness error.
    fn check_and_generate_subclaim(
        &self,
        asserted_sum: &F,
        mask_poly_nv: Self::MPNumV,
        mask_poly_degree: Self::MPDeg
    ) -> Result<Self::ZkSumCheckSubClaim, PolyIOPErrors>;
}

/// A ZkSumCheckSubClaim is a claim generated by the verifier at the end of
/// verification when it is convinced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ZkSumCheckSubClaim<F: PrimeField> {
    /// the multi-dimensional point that this multilinear extension is evaluated
    /// to
    pub point: Vec<F>,
    /// the expected evaluation
    pub expected_evaluation: F,
}

impl<F: PrimeField> ZkSumCheck<F> for PolyIOP<F> {
    type SumCheckProof = IOPProof<F>;
    type VirtualPolynomial = VirtualPolynomial<F>;
    type VPAuxInfo = VPAuxInfo<F>;
    type MultilinearExtension = Arc<DenseMultilinearExtension<F>>;
    type RandomMaskPolynomial = RandomMaskPolynomial<F>;
    type SumCheckSubClaim = ZkSumCheckSubClaim<F>;
    type Transcript = IOPTranscript<F>;
    type MPDeg = usize;
    type MPNumV = usize;

    fn extract_sum(proof: &Self::SumCheckProof) -> F {
        let start = start_timer!(|| "extract sum");
        let res = proof.proofs[0].evaluations[0] + proof.proofs[0].evaluations[1];
        end_timer!(start);
        res
    }

    fn init_transcript() -> Self::Transcript {
        let start = start_timer!(|| "init transcript");
        let res = IOPTranscript::<F>::new(b"Initializing SumCheck transcript");
        end_timer!(start);
        res
    }

    fn prove(
        poly: &Self::VirtualPolynomial,
        mask_poly: &Self::RandomMaskPolynomial,
        rho: &F,
        transcript: &mut Self::Transcript,
    ) -> Result<Self::SumCheckProof, PolyIOPErrors> {
        let start = start_timer!(|| "sum check prove");

        transcript.append_serializable_element(b"aux info", &poly.aux_info)?;

        let mut prover_state = ZkSumCheckProverState::prover_init(poly, mask_poly)?;
        let mut challenge = None;
        let mut prover_msgs = Vec::with_capacity(poly.aux_info.num_variables);
        for _ in 0..poly.aux_info.num_variables {
            let prover_msg =
                ZkSumCheckProverState::prove_round_and_update_state(&mut prover_state, rho, &challenge)?;
            transcript.append_serializable_element(b"prover msg", &prover_msg)?;
            prover_msgs.push(prover_msg);
            challenge = Some(transcript.get_and_append_challenge(b"Internal round")?);
            assert!(challenge.unwrap() != F::zero());
            assert!(challenge.unwrap() != F::one());
        }
        // pushing the last challenge point to the state
        if let Some(p) = challenge {
            prover_state.sum_check_prover_state.challenges.push(p)
        };

        end_timer!(start);
        Ok(IOPProof {
            point: prover_state.sum_check_prover_state.challenges,
            proofs: prover_msgs,
        })
    }

    fn verify(
        claimed_sum: F,
        proof: &Self::SumCheckProof,
        aux_info: &Self::VPAuxInfo,
        transcript: &mut Self::Transcript,
        mask_poly_nv: usize,
        mask_poly_degree: usize
    ) -> Result<Self::SumCheckSubClaim, PolyIOPErrors> {
        let start = start_timer!(|| "sum check verify");

        transcript.append_serializable_element(b"aux info", aux_info)?;
        let mut verifier_state = IOPVerifierState::verifier_init(aux_info);
        for i in 0..aux_info.num_variables {
            let prover_msg = proof.proofs.get(i).expect("proof is incomplete");
            transcript.append_serializable_element(b"prover msg", prover_msg)?;
            IOPVerifierState::verify_round_and_update_state(
                &mut verifier_state,
                prover_msg,
                transcript,
            )?;
        }

        let res = IOPVerifierState::check_and_generate_subclaim(&verifier_state, &claimed_sum, mask_poly_nv, mask_poly_degree);

        end_timer!(start);
        res
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use ark_secp256k1::Fr;
    use ark_ff::UniformRand;
    use ark_std::test_rng;

    fn test_sumcheck(
        nv: usize,
        num_multiplicands_range: (usize, usize),
        num_products: usize,
    ) -> Result<(), PolyIOPErrors> {
        let mut rng = test_rng();
        let mut transcript = <PolyIOP<Fr> as ZkSumCheck<Fr>>::init_transcript();

        let (poly, asserted_sum) =
            VirtualPolynomial::rand(nv, num_multiplicands_range, num_products, &mut rng)?;
        let (mask, sum) = RandomMaskPolynomial::rand(nv, num_multiplicands_range.1, &mut rng);
        let rho = Fr::rand(&mut rng);
        assert!(rho != Fr::from(0));
        let asserted_sum = asserted_sum + rho * sum; 
        let proof = <PolyIOP<Fr> as ZkSumCheck<Fr>>::prove(&poly, &mask, &rho, &mut transcript)?;
        let poly_info = poly.aux_info.clone();
        let mut transcript = <PolyIOP<Fr> as ZkSumCheck<Fr>>::init_transcript();
        let subclaim = <PolyIOP<Fr> as ZkSumCheck<Fr>>::verify(
            asserted_sum,
            &proof,
            &poly_info,
            &mut transcript,
            mask.evaluations.len(),
            mask.evaluations[0].len()-1
        )?;
        let res = poly.evaluate(&subclaim.point).unwrap() + rho * mask.eval(&subclaim.point)?; 
        assert!(
            res == subclaim.expected_evaluation,
            "wrong subclaim"
        );
        Ok(())
    }

    #[test]
    fn test_trivial_polynomial() -> Result<(), PolyIOPErrors> {
        let nv = 10;
        let num_multiplicands_range = (2, 6);
        let num_products = 2;

        test_sumcheck(nv, num_multiplicands_range, num_products)
    }
}
