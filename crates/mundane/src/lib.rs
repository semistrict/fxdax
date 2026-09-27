//! The subset of `mundane::hash` that Fxfs uses, implemented with `sha2`.

pub mod hash {
    use sha2::Digest as _;

    /// A hash function.
    pub trait Hasher: Default {
        type Digest: Digest;

        fn update(&mut self, bytes: &[u8]);
        fn finish(self) -> Self::Digest;
    }

    /// The output of a hash function.
    pub trait Digest: Sized {
        const DIGEST_LEN: usize;
        type Bytes: AsRef<[u8]>;

        fn bytes(&self) -> Self::Bytes;
    }

    macro_rules! hash {
        ($hasher:ident, $digest:ident, $len:literal) => {
            #[derive(Clone, Default)]
            pub struct $hasher(sha2::$hasher);

            #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
            pub struct $digest([u8; $len]);

            impl Hasher for $hasher {
                type Digest = $digest;

                fn update(&mut self, bytes: &[u8]) {
                    self.0.update(bytes);
                }

                fn finish(self) -> $digest {
                    $digest(self.0.finalize().into())
                }
            }

            impl Digest for $digest {
                const DIGEST_LEN: usize = $len;
                type Bytes = [u8; $len];

                fn bytes(&self) -> [u8; $len] {
                    self.0
                }
            }
        };
    }

    hash!(Sha256, Sha256Digest, 32);
    hash!(Sha512, Sha512Digest, 64);
}
