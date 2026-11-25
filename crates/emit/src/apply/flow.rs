//! Joins and program-proved loop counts for controlled emission.

use std::collections::BTreeSet;

use bennett_ir::{BinOp, Block, BlockId, Const, Function, Inst, Operand, Predicate, Slot, Target, Terminator};
use petgraph::algo::dominators::{Dominators, simple_fast};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::Reversed;

use super::symbolic::{Frame, Point, before_instruction_fault, visit_possible_targets};

pub(super) struct Flow<'a> {
    graph: DiGraph<(), ()>,
    joins: Dominators<NodeIndex>,
    loops: Vec<Option<ShiftLoop<'a>>>,
    exit: NodeIndex,
}

impl<'a> Flow<'a> {
    pub(super) fn analyze(frame: &Frame<'a>) -> Self {
        let function = frame.module.function(frame.callee);
        let mut graph = DiGraph::<(), ()>::new();
        let nodes: Vec<_> = function.blocks.iter().map(|_| graph.add_node(())).collect();
        let exit = graph.add_node(());
        for (index, block) in function.blocks.iter().enumerate() {
            let (_, term) = before_instruction_fault(block);
            match term {
                None | Some(Terminator::Return(_) | Terminator::Exit(_) | Terminator::Fault(_)) => {
                    graph.add_edge(nodes[index], exit, ());
                }
                Some(term) => visit_possible_targets(term, |target| {
                    graph.add_edge(nodes[index], nodes[target.block.index()], ());
                }),
            }
        }
        let joins = simple_fast(Reversed(&graph), exit);
        let doms = simple_fast(&graph, nodes[0]);
        let mut flow = Self { graph, joins, loops: Vec::new(), exit };
        flow.loops = function
            .blocks
            .iter()
            .enumerate()
            .map(|(index, _)| {
                let block = BlockId(index as u32);
                ShiftLoop::prove(function, &doms, block).or_else(|| ShiftLoop::prove_latch(function, &flow, block))
            })
            .collect();
        flow
    }

    // The nearest common continuation of routes to a stop. A route that bypasses
    // it can return, fault, exhaust the budget, or make the construction decline.
    pub(super) fn join(&self, block: BlockId) -> Option<Point> {
        let node = self.joins.immediate_dominator(NodeIndex::new(block.index()))?;
        (node != self.exit).then(|| Point::entry(BlockId(node.index() as u32)))
    }

    pub(super) fn can_finish(&self, block: BlockId) -> bool {
        self.joins.immediate_dominator(NodeIndex::new(block.index())).is_some()
    }

    pub(super) fn can_revisit(&self, block: BlockId, stops_at: impl Fn(BlockId) -> bool) -> bool {
        if stops_at(block) {
            return false;
        }
        let start = NodeIndex::new(block.index());
        let mut work: Vec<_> = self.graph.neighbors(start).collect();
        let mut seen = BTreeSet::new();
        while let Some(next) = work.pop() {
            if next == start {
                return true;
            }
            if !stops_at(BlockId(next.index() as u32)) && seen.insert(next) {
                work.extend(self.graph.neighbors(next));
            }
        }
        false
    }

    // A common block before either route stops would be lowered twice.
    // An entry boundary contributes no instructions. A boundary inside a
    // block still allows the instructions before it to be emitted.
    pub(super) fn shares_code(
        &self,
        left: BlockId,
        right: BlockId,
        stop_index: impl Fn(BlockId) -> Option<usize>,
    ) -> bool {
        let mut visited = vec![None; self.graph.node_count()];
        for (route, start) in [left, right].into_iter().enumerate() {
            let mut work = vec![NodeIndex::new(start.index())];
            while let Some(node) = work.pop() {
                if node == self.exit {
                    continue;
                }
                let stop = stop_index(BlockId(node.index() as u32));
                if stop == Some(0) {
                    continue;
                }
                let owner = &mut visited[node.index()];
                if let Some(previous) = *owner {
                    if previous != route {
                        return true;
                    }
                    continue;
                }
                *owner = Some(route);
                if stop.is_none() {
                    work.extend(self.graph.neighbors(node));
                }
            }
        }
        false
    }

    pub(super) fn shift_loop(&self, block: BlockId) -> Option<ShiftLoop<'a>> {
        self.loops[block.index()]
    }
}

#[derive(Clone, Copy)]
pub(super) struct BranchEquality {
    pub(super) slot: Slot,
    pub(super) value: Const,
    pub(super) when_true: bool,
}

impl BranchEquality {
    pub(super) fn in_block(block: &Block) -> Option<Self> {
        let Terminator::Branch { cond: Operand::Slot(cond), .. } = &block.term else { return None };
        block.insts.iter().find_map(|inst| {
            let Inst::Compare { pred: pred @ (Predicate::Eq | Predicate::Ne), dst, lhs, rhs, .. } = inst else {
                return None;
            };
            if dst != cond {
                return None;
            }
            let ((Operand::Slot(slot), Operand::Const(value)) | (Operand::Const(value), Operand::Slot(slot))) =
                (lhs, rhs)
            else {
                return None;
            };
            Some(Self { slot: *slot, value: *value, when_true: *pred == Predicate::Eq })
        })
    }
}

#[derive(Clone, Copy)]
pub(super) struct ShiftLoop<'a> {
    pub(super) width: usize,
    pub(super) zero_on_exit: Slot,
    pub(super) condition: &'a Operand,
    pub(super) continue_when_true: bool,
    pub(super) continuing: &'a Target,
    pub(super) exiting: &'a Target,
}

impl<'a> ShiftLoop<'a> {
    fn prove(function: &'a Function, doms: &Dominators<NodeIndex>, header: BlockId) -> Option<Self> {
        let block = function.block(header);
        let Terminator::Branch { cond: condition, if_true, if_false } = &block.term else {
            return None;
        };
        let BranchEquality { slot, value, when_true } = BranchEquality::in_block(block)?;
        if value.value() != 0 {
            return None;
        }
        let word = Operand::Slot(slot);
        let parameter = block.params.iter().position(|param| param.slot == slot)?;
        let continue_when_true = !when_true;
        let (continuing, exiting) = if continue_when_true { (if_true, if_false) } else { (if_false, if_true) };
        if exiting.block == header || continuing.block == exiting.block {
            return None;
        }

        // For every route from the nonzero outcome back to the header, the next value
        // must be this iteration's logical right shift of the tested block parameter.
        let mut seen = BTreeSet::new();
        let mut edges = vec![(header, continuing)];
        let mut found_back_edge = false;
        while let Some((source, target)) = edges.pop() {
            if target.block == header {
                let shifted = &target.args[parameter];
                // Validation guarantees that an operand's definition dominates this use.
                // Requiring the header to dominate the shift makes it a computation of the
                // current pass, including when the shift and back edge are in separate blocks.
                let shift_block = doms.dominators(NodeIndex::new(source.index()))?.find(|node| {
                    function.blocks[node.index()].insts.iter().any(|inst| {
                        matches!(
                            inst,
                            Inst::Binary { op: BinOp::Lshr, dst, lhs, rhs: Operand::Const(amount), .. }
                                if Operand::Slot(*dst) == *shifted && *lhs == word && amount.value() == 1
                        )
                    })
                })?;
                if !doms.dominators(shift_block)?.any(|node| node.index() == header.index()) {
                    return None;
                }
                found_back_edge = true;
            } else if target.block != exiting.block
                && seen.insert(target.block)
                && let (_, Some(term)) = before_instruction_fault(function.block(target.block))
            {
                visit_possible_targets(term, |edge| edges.push((target.block, edge)));
            }
        }

        found_back_edge.then_some(Self {
            width: value.width().bits() as usize,
            zero_on_exit: slot,
            condition,
            continue_when_true,
            continuing,
            exiting,
        })
    }

    fn prove_latch(function: &'a Function, flow: &Flow<'_>, latch: BlockId) -> Option<Self> {
        let block = function.block(latch);
        let Terminator::Branch { cond: condition @ Operand::Slot(cond), if_true, if_false } = &block.term else {
            return None;
        };
        // A rotated loop tests the old word at the end of its body. Testing
        // old < 2 or old > 1 is equivalent to testing (old >> 1) against zero.
        let (continue_when_true, width, word) = block.insts.iter().find_map(|inst| {
            let Inst::Compare { pred, dst, width, lhs, rhs: Operand::Const(limit) } = inst else { return None };
            if dst != cond {
                return None;
            }
            let continue_when_true = *pred == Predicate::Ugt && limit.value() == 1;
            let continue_when_false = *pred == Predicate::Ult && limit.value() == 2;
            (continue_when_true || continue_when_false).then_some((continue_when_true, *width, lhs))
        })?;
        let (continuing, exiting) = if continue_when_true { (if_true, if_false) } else { (if_false, if_true) };
        if continuing.block == exiting.block || exiting.block == latch {
            return None;
        }
        let header = function.block(continuing.block);
        let parameter = header.params.iter().position(|param| Operand::Slot(param.slot) == *word)?;
        let Operand::Slot(shifted) = continuing.args[parameter] else { return None };
        if !block.insts.iter().any(|inst| {
            matches!(
                inst,
                Inst::Binary { op: BinOp::Lshr, dst, lhs, rhs: Operand::Const(amount), .. }
                    if *dst == shifted && lhs == word && amount.value() == 1
            )
        }) || flow.can_revisit(continuing.block, |block| block == latch)
        {
            // The body cannot replace the header parameter along another back edge.
            return None;
        }
        Some(Self {
            width: width.bits() as usize,
            zero_on_exit: shifted,
            condition,
            continue_when_true,
            continuing,
            exiting,
        })
    }
}
