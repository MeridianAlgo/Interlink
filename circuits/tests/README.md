# circuits/tests

This folder is for tests for the `circuits` crate.

## In plain terms

These tests ensure the circuits in `circuits/src` satisfy their constraints (i.e., they can be proven/verified and the math matches expectations).

## Notes

Most tests in this repo currently live next to the circuit code under `#[cfg(test)]` modules, but this directory is the conventional place for larger integration-style circuit tests.
