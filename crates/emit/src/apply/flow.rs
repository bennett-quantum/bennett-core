//! Joins and program-proved loop counts for controlled emission.

use std::collections::BTreeSet;

use bennett_ir::{BinOp, BlockId, Function, Inst, Operand, Predicate, Target, Terminator};
use petgraph::algo::dominators::{Dominators, simple_fast};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::Reversed;

use super::symbolic::{Frame, Point};

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
            block.term.visit_targets(|target| {
                graph.add_edge(nodes[index], nodes[target.block.index()], ());
            });
            if matches!(block.term, Terminator::Return(_)) {
                graph.add_edge(nodes[index], exit, ());
            }
        }
        let joins = simple_fast(Reversed(&graph), exit);
        let doms = simple_fast(&graph, nodes[0]);
        let loops = function
            .blocks
            .iter()
            .enumerate()
            .map(|(index, _)| ShiftLoop::prove(function, &doms, BlockId(index as u32)))
            .collect();
        Self { graph, joins, loops, exit }
    }

    // The nearest common continuation of routes to a return. A route that cannot
    // reach it will instead return, exhaust the budget, or make the construction decline.
    pub(super) fn join(&self, block: BlockId) -> Option<Point> {
        let node = self.joins.immediate_dominator(NodeIndex::new(block.index()))?;
        (node != self.exit).then(|| Point::entry(BlockId(node.index() as u32)))
    }

    pub(super) fn can_return(&self, block: BlockId) -> bool {
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
pub(super) struct ShiftLoop<'a> {
    pub(super) width: usize,
    pub(super) condition: &'a Operand,
    pub(super) continue_when_true: bool,
    pub(super) continuing: &'a Target,
    pub(super) exiting: &'a Target,
}

impl<'a> ShiftLoop<'a> {
    fn prove(function: &'a Function, doms: &Dominators<NodeIndex>, header: BlockId) -> Option<Self> {
        let block = function.block(header);
        let Terminator::Branch { cond: condition @ Operand::Slot(cond), if_true, if_false } = &block.term else {
            return None;
        };
        let (continue_when_true, width, word) = block.insts.iter().find_map(|inst| {
            let Inst::Compare { pred, dst, width, lhs, rhs } = inst else { return None };
            if dst != cond || !matches!(pred, Predicate::Eq | Predicate::Ne) {
                return None;
            }
            let (word, zero) = if matches!(lhs, Operand::Slot(_)) { (lhs, rhs) } else { (rhs, lhs) };
            matches!(zero, Operand::Const(value) if value.value() == 0).then_some((
                *pred == Predicate::Ne,
                *width,
                word,
            ))
        })?;
        let parameter = block.params.iter().position(|param| Operand::Slot(param.slot) == *word)?;
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
                                if Operand::Slot(*dst) == *shifted && lhs == word && amount.value() == 1
                        )
                    })
                })?;
                if !doms.dominators(shift_block)?.any(|node| node.index() == header.index()) {
                    return None;
                }
                found_back_edge = true;
            } else if target.block != exiting.block && seen.insert(target.block) {
                function.block(target.block).term.visit_targets(|edge| edges.push((target.block, edge)));
            }
        }

        found_back_edge.then_some(Self {
            width: width.bits() as usize,
            condition,
            continue_when_true,
            continuing,
            exiting,
        })
    }
}
