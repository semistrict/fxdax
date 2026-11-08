# What the guide is built on, and what it leaves out

The guide is for one reader: an expert systems programmer who owns this codebase and hasn't read
it. Its job is to let him **predict what the code does**, from a dozen invariants, the decisions
behind them, and fast checks of any claim against the code, tests, specs and traces.

Most learning research studies novices: undergraduates, toy material, a quiz straight afterwards.
Each finding below was weighed for this reader before the design used it.

## What the design uses

- **Predict before the reveal.** Every reveal in a mission follows a prediction or a design, and
  the wrong options name a real misconception.
  - Source: pretesting, St. Hilaire et al. 2024, a preregistered meta-analysis. g = 0.54 for the
    material asked about, about 0 for material not asked about; g = 0.65 when the learner
    generates an answer, against 0.22 when they only read the question.
  - Why it fits an expert: forming a hypothesis and checking it against the code is how experts
    read code anyway.
- **Design it before seeing ours.** Page Pilot's fsync step asks for your sequence before showing
  fxdax's.
  - Source: problem solving before instruction, Sinha & Kapur 2021, 53 studies: g = 0.36, up to
    0.58 for designs that follow productive-failure principles. It reverses for young children
    and for domain-general skills.
  - Why it fits: an expert's own design is the best contrast for fxdax's.
- **Remove a rule, see what breaks.** The safety-catch lab removes a rule from the TLA+ model and
  replays TLC's counterexample.
  - Source: interactive worked examples, Tithi et al. 2026, n = 155. Find-and-fix examples helped
    high-prior-knowledge learners most.
  - The material is real: specs/*/mutations and guest/mutants.
- **The code stays one step away.** Diagrams are labelled as illustrations, every claim cites the
  code that enforces it, and Page Pilot ends with a timed, diagram-free search for the code.
  - Source: Sibia et al., ICER 2026, gaze tracking with n = 19. Learners spent about half their
    time on the code itself even with visual views beside it, wanted control of the abstraction
    level, and distrusted metaphors.
- **Content from the repo's own experts.** The guide states the design docs' invariants as they
  state them, and draws on the dead ends, the mutants' stories and the specs' rules. It writes no
  generic prose.
  - Source: LACY, 2026, on a 30K-line legacy system: expert-curated tours scored 83% on quizzes,
    AI-only tours 57%.
- **No explain-it-to-me chatbot.** The atlas's prompt asks Claude Code to make the reader predict
  first, cite code by snippet, and name the check that would fail.
  - Source: Shen & Tamkin 2026, an RCT. Delegating to AI cut comprehension. Asking conceptual
    questions, or for explanations, scored 65–86%, against 24–39%.
  - Source: Copilot brownfield study, 2025. Verification loops went with comprehension.
- **Link code to its docs, both ways.** Every invariant names its doc, its code and its evidence.
  - Source: Code Compass, 2024. Developers struggled to connect code to its documentation, and
    more information alone didn't improve comprehension.

## What it leaves out, for this reader

- **Flashcards and spaced repetition of names.** An expert forgets a name and looks it up. The
  model behind the names is the goal, and the index is the lookup.
- **Quizzes as the main mode, and stacked self-explanation prompts.** Heavy guidance helps
  novices (d = 0.51) and costs experts (d = −0.43 when mismatched): expertise reversal, Tetzlaff et
  al. 2025, 60 experiments. Self-explanation prompts added to worked examples hurt in maths
  (Barbieri 2023). The guide asks a few pointed questions, not many.
- **Metaphors, cartoons, sonification.** The guide uses the system's own vocabulary and its real
  states.

## What is still a guess

The guide hasn't been tested on its reader. Page Pilot ends with the test that matters: the
diagram-free search for the code, timed. A day later, can he predict three cases he hasn't seen?
Phase 2 waits on that answer.

## Sources

- St. Hilaire et al. 2024, the prequestion effect: https://www.bohrium.com/paper-details/guessing-as-a-learning-intervention-a-meta-analytic-review-of-the-prequestion-effect/903820706179973241-12382
- Sinha & Kapur 2021, productive failure (Review of Educational Research): https://www.x-mol.com/paper/1406031499744456704
- Tetzlaff et al. 2025, the expertise reversal effect: https://www.pedocs.de/frontdoor.php?la=en&source_opus=34113
- Tithi et al. 2026, interactive worked examples: https://arxiv.org/abs/2602.16806
- Sibia et al. 2026, code as anchor: https://arxiv.org/abs/2606.19570
- Shen & Tamkin 2026, how AI impacts skill formation: https://arxiv.org/html/2601.20245v1
- Copilot comprehension in brownfield code, 2025: https://arxiv.org/pdf/2511.02922
- LACY 2026: https://arxiv.org/abs/2603.25391
- Code Compass 2024: https://arxiv.org/pdf/2405.06271
- Bisra 2018, self-explanation: https://doi.org/10.1007/s10648-018-9434-x
