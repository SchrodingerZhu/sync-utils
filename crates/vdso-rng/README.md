# vDSO-RNG

A rust wrapper of linux vDSO random generator.

Unfortunately, rustix does not expose enough APIs for us to access vDSO `getrandom`. This crate provides an 
alternative wrapper for such functionalities. We choose to do it in a dependency-free way, without assuming
any support from libc.