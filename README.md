# Oscar's Emporium

Welcome one and all to Oscar's Emporium! Are you a fan of garbage or garbage
collection? Then look no further! You've come to the right place!

Jokes aside, this repository is meant to serve as a testing ground for memory
management and garbage collection experiments for Boa.

## Open questions

 - What should the GC API be?
     - Is it possible to support multiple GCs via a common API?
 - How should memory allocation be handled?
     - What is best for JavaScript performances?

## GC API investigation

The current API model investigation for Boa issue #2631 is documented in
[`notes/gc_api_models.md`](./notes/gc_api_models.md).

## Project structure

The current project structure is as follows.

  - `src`: contains all code releated to GCs. 
  - `notes`: experiment and research notes on GC and GC related things

