# Fixture resource limits

This is a fixture-local safety proposal, not a final AIP language or wire-protocol decision. It implements the `PRINCIPLES.md` §4 gates for bounded definition parsing while preserving the five existing fixture forms and their semantics.

| Budget | Hard ceiling | Applies to |
|---|---:|---|
| Source bytes | 1 MiB | Every `load_str` form, including host extraction and raw A input |
| Tokens | 65,536 significant tokens, plus the lexer EOF sentinel | Both the A lexer and TS/Python host tokenizer; reject before appending the next token |
| Expression parser nesting | 64 active expression frames | Parentheses, `not`, calls, and nested expressions; the initial expression frame counts, so 63 ordinary nested wrappers remain accepted |
| Expression AST nodes | 4,096 per root expression | Count each expression node as the parser creates it; reject before creating node 4,097 |
| Recursive AST depth | 64 container nodes | Iteratively measure `not`, boolean containers, calls, comparisons, membership, and `exists` before semantic analysis |
| Executable path segments | 64 field steps per path | Semantic check for policy roots and predicate bodies; count each lowered path step in expanded-node work |
| Expanded policy depth | 64 weighted container/call levels | Maximum structural expression depth after named predicate bodies are inlined; a call adds its AST container depth to the callee's expanded depth |
| Expanded policy nodes | 16,384 total units | Saturating sum of expression nodes and path field steps across predicate bodies and all policy expression roots after named predicate expansion |
| Host definition literal nesting | 64 active literal values | Nested arrays/objects in H forms; the outer `define` object is the root and existing 63-array fixture shape remains accepted |

Before this change, source size and token count were unbounded, flat `and`/`or` vectors had no AST node limit, and selected recursive paths had a 128-frame guard. A valid fixture predicate expanded to 10,000 `true and` operands is below the byte and token ceilings and was accepted before the expression-node budget. The AST uses `And(Vec<Expr>)`, so that case did not crash the default-stack probe; it was an unbounded-width regression, not an observed stack overflow. The 4,096-node per-expression budget now rejects it while the parser is building the vector. Actual recursive depth is measured from the built AST using an explicit heap stack; the parser's existing active-frame guard also bounds how deep an AST can be built before this check. A flat path remains one parser AST leaf, so the standalone expression parser still accepts long paths under byte/token limits. Executable policy paths receive a separate 64-field-step semantic ceiling because SQL lowering expands each relationship step into nested SQL; every step also contributes to expanded-node work.

## §4 design gates

1. **Principle served / tension:** Serves principles 2 and 4 by making server-side definition parsing predictably bounded and reviewable. The limits constrain unusually large definitions, so their exact values remain fixture proposals.
2. **Expression range:** Definitions under the hard budget retain their expression range. Oversized or deeply nested definitions are rejected explicitly.
3. **Application server work:** Reduces the need for each embedding application to invent its own parser resource caps. It does not add per-application declarations.
4. **Server authority / trust:** Definition source is untrusted. The parser checks byte count, token count, and nesting before semantic analysis; no caller-supplied budget is trusted.
5. **JS/TS and Python experience:** Both host extractors receive the same byte and token ceilings. The five existing forms and their generated execution/metadata remain equivalent.
6. **One representation:** The same fixed ceilings apply regardless of source form; no alternate bypass syntax or per-form budget knob is added.
7. **Means vs. ends:** These checks bound implementation resource use. They do not establish an IR, compiler phase, or parser syntax as an AIP goal.
8. **Founder decision / Open:** `docs/DECISIONS.md` does not specify production numeric parser budgets. These values are local spike defaults only and do not decide the production protocol's compatibility policy or configurable budget API.

## Validation intent

New tests cover each entry path, exact accepted nesting, the byte/token boundaries, 10,000-operand fixture predicates, expression node boundaries, flat paths, recursive AST depth, predicate call chains, call-graph diamonds, and a small-stack child process. The parser completes supported local nesting on a 1 MiB worker stack. Process-isolated probes contain any future stack regression. The 4,096 local-node ceiling limits one parsed expression; the 16,384 expanded-node ceiling separately limits total inlining work.

## Predicate dependency graph

The call graph is measured with an iterative expression walk and validated by an iterative three-color topological traversal in O(V + E) expected time. A back edge retains the `POLICY_CYCLE` diagnostic. In callee-before-caller order, the analyzer computes each predicate's expanded node count and weighted depth, saturating node arithmetic at one above the hard ceiling. It rejects a predicate expansion deeper than 64 or larger than 16,384 nodes. It then adds each predicate body and each policy expression root to the shared 16,384-node budget; roots include access guards, limits, resource read/check expressions, aggregate filters and arguments, transition conditions and effects, and expose/create/compose/apply expressions. This bounds the inlining cost that the downstream V2 SQL generator otherwise repeats when it recursively expands named predicate bodies. The source and token ceilings still independently bound graph input size.

Two process-isolated RED probes confirmed the previous implementation accepted a 1,000-edge predicate chain and a 30-level diamond graph while both sources stayed below the byte and token ceilings. The GREEN implementation returns `POLICY_EXPANSION_LIMIT` for both. The probes did not observe a stack crash; they demonstrate that excessive compiler expansion was accepted without an explicit budget.
