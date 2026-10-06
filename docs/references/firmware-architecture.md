# Reference index — firmware architecture for a low-power I²C-slave supervisor

Curated, cited sources on how to architect firmware for exactly our job: an MSP430-class **low-power
supervisor** that sleeps in LPM, wakes on interrupts (I²C-slave + analog/GPIO triggers), reflects state
over a register-map, and switches between an **active** (Leaf-master) and **passive** (reactive, asleep)
mode. Gathered 2026-10-06 (research for `DEEPSLEEP-EXTRACTION.md` Phase 1b). Each entry: what it says,
the load-bearing quote, the URL, and whether it's consensus or one author's opinion.

**Why this file exists:** the bench/CI may be offline and vendor blogs rot or bot-wall. Durable
provenance for the architecture decisions in `DEEPSLEEP-EXTRACTION.md` / `adr/0013` lives here, not only
in chat. PDFs we could save are under [`pdf/`](pdf/); hardware datasheets are in [`../../datasheets/`](../../datasheets/INDEX.md).

---

## Saved locally (PDF)
- **Samek, *Modern Embedded Systems Programming: Beyond the RTOS*** → [`pdf/Beyond-the-RTOS.pdf`](pdf/Beyond-the-RTOS.pdf)
  (42 pp). The active-object / event-driven run-to-completion (RTC) argument.
- **Samek, *Use an MCU's low-power modes in foreground/background*** (EE Times, 2007) →
  [`pdf/Samek0710.pdf`](pdf/Samek0710.pdf) (4 pp). The canonical "sleep in the idle condition" + the
  MSP430 `__bis_SR_register(LPMx|GIE)` atomic-sleep idiom. **Most directly applicable single source.**

## Indexed by URL (not saved — bot-walled or HTML blog; quotes captured here)
Protothreads paper is bot-walled (HTTP 466); captured by quote below.

---

## 1. Where the "enter LPM" decision lives — **strong consensus**
Sleep belongs at the one point reached only when all work is done: the bottom of the loop / the idle
hook. "Do work on wake; drop to LPM when everything is Pending; wake on interrupt" is the standard shape.

- **Samek**, `Samek0710.pdf` (saved): *"To employ a low-power MCU mode, the background loop must first
  determine that all external and internal events have been processed so that the CPU clock can be
  stopped until the next external event (an interrupt) will wake the CPU up. This situation is called the
  idle condition."* His MSP430 listing enters LPM with `__bis_SR_register(LPM1_bits|GIE)` and the ISR
  clears the LPM bits on exit *only when there is work*.
- **The atomicity trap (load-bearing for us):** Samek — *"Entering a sleep mode while interrupts are
  disabled poses a chicken-and-egg problem for waking the system up, because only an interrupt can
  terminate the low-power sleep mode."* On MSP430 the fix is free: setting LPM+GIE is one instruction, so
  there is no window between "decide to sleep" and "an interrupt can wake me." → the lost-wakeup race is
  avoided by construction.
- **QP/C QV cooperative kernel docs** — https://www.state-machine.com/qpc/arm-cm_qv.html : the idle
  callback `QV_onIdle()` *"must be invoked with interrupts disabled, because the idle condition can be
  changed by any interrupt that posts events"*, then `QV_CPU_SLEEP()` *"atomically go[es] to sleep and
  enable[s] interrupts."* This is the exact shape for our `sched` idle→LPM pender (ADR-0013's unbuilt
  "real waker").
- **Beningo**, *Sleep On Exit* — https://www.beningo.com/tips-and-tricks-sleep-on-exit/ : *"keep the
  system in a sleep mode at all times and only wake-up to run a single interrupt service routine."*
- **FreeRTOS tickless idle** — https://www.freertos.org/low-power-tickless-rtos.html : the RTOS form of
  the same idea.

## 2. ISR does the minimum; defer to the main loop — **very strong consensus**
- **Ganssle**, *Coding ISRs* — https://www.ganssle.com/articles/acodeisr.htm : *"The biggest mistake made
  in ISRs … is to put too much hardware service in the ISR itself,"* and consider *"using the ISR to
  spawn off a different task that handles the bulk of the work."*
- **Koopman**, *Rules for Using Interrupts* — https://betterembsw.blogspot.com/2013/03/rules-for-using-interrupts.html :
  *"Keep ISR execution time very short. 100-200 clock cycles tops … If you have a lot of work to do,
  shovel the data into a holding buffer and let the main loop or a non-ISR task do the rest."* Also:
  *"Acknowledge the interrupt source at the beginning of the ISR."*
- Maps to our `SlaveEvent` seam: the ISR wakes + hands off; `deepsleep::stem` (main) does the work.

## 3. ISR→main handoff: volatile flag / lock-free SPSC ring — **strong**
- **Barr Group / Nigel Jones**, *C's Volatile Keyword* — https://barrgroup.com/embedded-systems/how-to/c-volatile-keyword :
  a flag shared ISR↔main must be `volatile` or the compiler caches it; **but** `volatile` ≠ atomic /
  thread-safe.
- **Downtown Doug Brown**, *Interrupt-safe ring buffers* —
  https://www.downtowndougbrown.com/2013/01/microcontrollers-interrupt-safe-ring-buffers/ : *"If the
  interrupt only modifies the head, and the main loop only modifies the tail, there's no conflict …
  requiring disabling interrupts."* Indices must be `volatile` and index writes atomic (*"the native size
  of your architecture"* → **16-bit on MSP430, not 32-bit**). Caveat he states: multi-core/reorder needs
  *"a memory barrier between writing the queue data and updating the index"* — on single-core MSP430 this
  is a **compiler**-ordering concern (write payload before publishing index), not a hardware one.

## 4. ISR masks its own source; main re-enables after servicing — **moderate (assembled)**
A recognized idiom for level-held conditions the ISR can't cheaply clear: mask that one source, wake,
return; main services then re-enables. **Distinct from** re-enabling *global* interrupts in an ISR, which
is forbidden:
- **Koopman**, *Do Not Re-Enable Interrupts In An ISR* —
  https://betterembsw.blogspot.com/2014/01/do-not-re-enable-interrupts-in-isr.html : *"Don't re-enable
  interrupts within an ISR. That's just asking for subtle race condition and stack overflow problems"* —
  masking your **own** source (not GIE) and deferring re-enable to main is consistent with this rule.
- This is our chosen 1b ISR shape (`DEEPSLEEP-EXTRACTION.md` §A refinement): ISR masks eUSCI IE + sets a
  wake flag, main re-enables after `next_event()` drains.

## 5. Keep application state OUT of the ISR — **very strong**
- **Barr Group (BARR-C:2018)** — https://barrgroup.com/sites/default/files/barr_c_coding_standard_2018.pdf :
  `volatile` for ISR-shared globals, **and** *"whenever a variable is declared volatile, it shall always
  be protected from race conditions at each location of access."*
- **Design implication (textbook-validates our 1a):** keep the `RegFile` solely in the main dispatcher;
  let the ISR touch only the wake flag (and, if a byte queue is ever added, only its own index). Then **no
  critical section** is needed around the register file. This is exactly the 1a split.

## 6. I²C-slave + sleep specifics — **mechanism strong (TI); stretch-latency synthesized**
- **TI SLAU445 (eUSCI, FR2xx/4xx)** → saved at [`../../datasheets/User's Guide slau445i.pdf`](../../datasheets/):
  *"operate the eUSCI_B in I²C slave mode while the device is in LPM4 and all internal clock sources are
  disabled. The receive or transmit interrupts can wake up the CPU from any low-power mode."* — the
  authority for our Passive-sleep wake-on-Stem-slave. (LPM4 if the master supplies SCL and we need no
  internal clock; LPM3 if a low-freq timer must keep running.)
- **Clock stretching** covers wake latency: the slave holds SCL low across the address ACK until firmware
  is ready. Hazard: a master that does **not** support stretching — Silabs **AN1095** (I²C master does
  not support clock stretching) — https://www.silabs.com/documents/public/application-notes/an1095-i2c-master-does-not-support-clock-stretching.pdf
  (403 on scripted fetch — indexed by title/role, body not read). Our master is the RP2040 stem-probe /
  M7, both of which stretch, so this is low-risk here.
- **Two-tier servicing** (field-reported, forums — corroborating not authoritative): keep the time-critical
  byte shuffle minimal, defer frame/command interpretation to main; over-deferring the byte level risks
  over-stretch / RX overrun. For us this only bites in **Active** (busy loop); in **Passive** (the sleep
  mode) the loop is otherwise idle, so `next_event()` reading RXBUF/TXBUF in main right after the wake ISR
  is fine. Revisit if Active ever needs to answer the Stem master mid-Leaf-scan.

## 7. Structure: event-driven RTC state machine over blocking superloop — **pattern = consensus; "framework beats all" = Samek's opinion**
- **Samek**, `Beyond-the-RTOS.pdf` (saved) / https://www.embeddedrelated.com/showarticle/951.php :
  tasks as *"'message pumps', with just one blocking call at the top of the task loop, which waits
  generically for all events,"* and event handlers *"are not allowed to block, but must quickly return to
  the 'message pump'"* (= run-to-completion). His stronger claim — active-object *frameworks* beat RTOS
  and superloop — is **his opinion** (he sells QP); adopt the **pattern**, not necessarily the framework,
  on our 15.5 KB FRAM budget.
- **Memfault**, *A Simple Scheduler via an Interrupt-driven Actor Model* —
  https://interrupt.memfault.com/blog/hardware-actor-scheduler : frames it neutrally as a *"middle
  ground"* between superloop and RTOS.
- **Blocking is debt:** Samek — https://www.embeddedrelated.com/showarticle/1704.php : *"Removing blocking
  calls sprinkled throughout the code is like trying to 'unscramble' scrambled eggs."*

## 8. Modes as an explicit FSM with entry/exit actions — **very strong consensus**
- **Embedded Artistry**, *Software Layering* — https://embeddedartistry.com/fieldatlas/16737/ : a state
  machine *"obligates the program to always be in a clear and consistent state."*
- **Ganssle**, *The Firmware Handbook* (chapters: Embedded/Hierarchical State Machine Implementation).
- **Samek**, *Practical UML Statecharts* — entry/exit actions make a sequenced handoff (our
  Passive→Active Leaf acquire/release) deterministic: exit(old) then entry(new).
- Maps to our `mode::ModeMachine` — one `Mode` enum, a transition fn running release/acquire as exit/entry.

## 9. Transport ↔ dispatch ↔ state layering — **strong consensus**
- **Beningo**, *3 Strategies for a Successful Firmware Architecture* —
  https://www.beningo.com/3-strategies-for-developing-a-successful-firmware-architecture/ : *"abstracts
  away all those low-level details. It separates the application from the hardware,"* layering by
  *"responsibilities rather than hardware quirks."*
- **Embedded Artistry** four-layer model (Processor / HW-Platform / Platform / Software).
- **Validates our crate split exactly:** `bsp::stem_i2c` (transport = bytes) → `deepsleep::stem` (dispatch
  = meaning) → `deepsleep::mode` (state). The ISR knows nothing of modes; the FSM knows nothing of I²C
  timing; the command/FSM layer unit-tests on-host with no bus.

## 10. Stackless cooperative tasks (protothreads / Rust async) — recognized lightweight RTOS alternative
- **Dunkels, *Protothreads*** — https://dunkels.com/adam/pt/about.html (paper
  https://dunkels.com/adam/dunkels06protothreads.pdf, **bot-walled 466, not saved**): *"a blocking context
  on top of an event-driven system … only two bytes of memory per protothread."* Known sharp edge:
  **locals are not preserved across a yield** (switch/Duff's device) → use persisted state.
- **Rust async / `no_std`**: the modern embodiment — `async fn` compiles to a stackless state machine the
  executor polls; **does** preserve locals across `.await`. This is our `sched` crate (ADR-0013): the
  compiler generates the state machine protothreads make you hand-write.

---

### Net architecture this points to (see `DEEPSLEEP-EXTRACTION.md` for the applied plan)
1. Superloop-as-message-pump: one idle point; **Passive idle → LPM entered atomically with GIE**.
2. ISRs minimal (wake + mask / ack); `RegFile` stays in main → no critical sections.
3. `Mode` = explicit FSM (Active/Passive) with acquire/release as entry/exit.
4. Layers: `bsp::stem_i2c` (bytes) → `deepsleep::stem` (dispatch) → `deepsleep::mode` (state). ✅ already 1a.
5. Watchdog: **never pet from a dumb timer ISR** (Koopman/Ganssle/MISRA/Memfault). **Grounded in
   slau445i §12 (2026-10-06):** reset-mode WDT_A has a clock fail-safe (ACLK/SMCLK loss → auto VLOCLK),
   so deep LPM does NOT stop it — it keeps counting and resets the sleeper; and watchdog mode blocks
   LPMx.5. → Decision: arm the backstop only while awake/processing, `WDTHOLD` across LPM, re-arm on
   wake (a sleeping CPU can't hang). This is both TI's low-power guidance and Koopman's "kick tied to
   real work." See `DEEPSLEEP-EXTRACTION.md` Phase 1b Design point 1.
