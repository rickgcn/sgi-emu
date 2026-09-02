# sgi-emu

A work-in-progress emulator for Silicon Graphics workstations.

The project is currently focused on the original **SGI Indigo (IP12)** with a 33 MHz MIPS R3000A processor. The current milestone is to boot IRIX 5.3 using a maintainable model of software-visible hardware behavior.

> [!IMPORTANT]
> sgi-emu is under active development. IRIX does not boot yet.

## Current status

The Indigo IP12 PROM currently boots far enough to enter the PROM monitor
and identify the emulated machine:

```text
>> hinv -v
              Memory size:  8 Mbytes
   Instruction cache size: 32 Kbytes
  Instruction refill size: 16 words
    Instruction streaming: Enabled
          Data cache size: 32 Kbytes
         Data refill size: 4 words
      Partial word stores: Enabled
                SCSI Disk: dksc(0,1)
               SCSI CDROM: Controller 0, ID 4
                CPU board: IP12 33 MHz, revision 0, with FPU
```

Implemented sufficiently for the current PROM path:

- R3000A CPU
- CP0 / exceptions / TLB
- FPU
- Caches
- Basic IP12 memory/platform support
- Basic SCSI device discovery

Still under development:

- PIC1
- HPC1
- INT2
- SCSI disk/CD-ROM I/O
- Ethernet
- Audio
- Graphics
- IRIX boot

## Emulation philosophy

sgi-emu focuses on reproducing **software-visible hardware behavior**.

The goal is not to reproduce every internal pipeline, arbitration mechanism, or silicon implementation detail unless software can observe the difference.

In short:

> **Emulate observables, not mechanisms.**

Timing is modeled when it is architecturally or software-visible, such as timers, interrupts, timeouts, DMA completion, or required busy states.

The global scheduler models time and externally observable events rather than device implementation details.

## Machines

| Machine        | Platform | Status                 |
| -------------- | -------- | ---------------------- |
| Indigo (R3000) | IP12     | Active development     |
| Indigo (R4000) | IP20     | Possible future target |
| Indy           | IP24     | Possible future target |
| Indigo2        | IP22     | Possible future target |
| O2             | IP32     | Possible future target |

## Accuracy and validation

Hardware behavior is validated using a combination of:

- SGI hardware and software documentation
- original PROM behavior
- IRIX software behavior and available diagnostics
- available hardware specifications
- open-source operating system drivers
- independent emulator/reference implementations where useful

Other emulator implementations are treated as references, not as hardware specifications.

When documentation and existing implementations disagree, preference is given to reproducible behavior and primary sources.

## Getting started

### Building

```bash
git clone --recursive https://github.com/rickgcn/sgi-emu.git
cd sgi-emu
cargo build --release
```

### Running

```bash
cargo run --release
```

## AI-assisted development

Coding agents and language models are used extensively during development.

They are tools for implementation, source discovery, testing, and code review; hardware behavior is not considered correct solely because an AI generated or suggested it.

The project maintainer remains responsible for architectural decisions, hardware modeling, validation, and the resulting code.

## Firmware and software

sgi-emu does not distribute IRIX installation media.

Original SGI firmware may be required for some machines. Users are responsible for obtaining firmware and operating-system media appropriate for their system and jurisdiction.

## License

sgi-emu is licensed under the GNU General Public License v3.0. See [LICENSE](LICENSE) for details.