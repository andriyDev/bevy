use crate::{
    render_graph::{
        Edge, InputSlotError, OutputSlotError, RenderGraphContext, RenderGraphError,
        RunSubGraphError, SlotInfo, SlotInfos,
    },
    render_phase::DrawError,
    renderer::RenderContext,
};
pub use bevy_ecs::label::DynEq;
use bevy_ecs::{
    define_label,
    intern::Interned,
    query::{QueryItem, QueryState, ReadOnlyQueryData},
    world::{FromWorld, World},
};
use core::fmt::Debug;
use downcast_rs::{impl_downcast, Downcast};
use thiserror::Error;
use variadics_please::all_tuples_with_size;

pub use bevy_render_macros::RenderLabel;

use super::{InternedRenderSubGraph, RenderSubGraph};

define_label!(
    #[diagnostic::on_unimplemented(
        note = "consider annotating `{Self}` with `#[derive(RenderLabel)]`"
    )]
    /// A strongly-typed class of labels used to identify a [`Node`] in a render graph.
    RenderLabel,
    RENDER_LABEL_INTERNER
);

/// A shorthand for `Interned<dyn RenderLabel>`.
pub type InternedRenderLabel = Interned<dyn RenderLabel>;

pub trait IntoRenderNodeArray<const N: usize> {
    fn into_array(self) -> [InternedRenderLabel; N];
}

macro_rules! impl_render_label_tuples {
    ($N: expr, $(#[$meta:meta])* $(($T: ident, $I: ident)),*) => {
        $(#[$meta])*
        impl<$($T: RenderLabel),*> IntoRenderNodeArray<$N> for ($($T,)*) {
            #[inline]
            fn into_array(self) -> [InternedRenderLabel; $N] {
                let ($($I,)*) = self;
                [$($I.intern(), )*]
            }
        }
    }
}

all_tuples_with_size!(
    #[doc(fake_variadic)]
    impl_render_label_tuples,
    1,
    32,
    T,
    l
);

/// A render node that can be added to a [`RenderGraph`](super::RenderGraph).
///
/// Nodes are the fundamental part of the graph and used to extend its functionality, by
/// generating draw calls and/or running subgraphs.
/// They are added via the `render_graph::add_node(my_node)` method.
///
/// To determine their position in the graph and ensure that all required dependencies (inputs)
/// are already executed, [`Edges`](Edge) are used.
///
/// A node can produce outputs used as dependencies by other nodes.
/// Those inputs and outputs are called slots and are the default way of passing render data
/// inside the graph. For more information see [`SlotType`](super::SlotType).
pub trait Node: Downcast + Send + Sync + 'static {
    /// Specifies the required input slots for this node.
    /// They will then be available during the run method inside the [`RenderGraphContext`].
    fn input(&self) -> Vec<SlotInfo> {
        Vec::new()
    }

    /// Specifies the produced output slots for this node.
    /// They can then be passed one inside [`RenderGraphContext`] during the run method.
    fn output(&self) -> Vec<SlotInfo> {
        Vec::new()
    }

    /// Updates internal node state using the current render [`World`] prior to the run method.
    fn update(&mut self, _world: &mut World) {}

    /// Runs the graph node logic, issues draw calls, updates the output slots and
    /// optionally queues up subgraphs for execution. The graph data, input and output values are
    /// passed via the [`RenderGraphContext`].
    fn run<'w>(
        &self,
        graph: &mut RenderGraphContext,
        render_context: &mut RenderContext<'w>,
        world: &'w World,
    ) -> Result<(), NodeRunError>;
}

impl_downcast!(Node);

#[derive(Error, Debug, Eq, PartialEq)]
pub enum NodeRunError {
    #[error("encountered an input slot error")]
    InputSlotError(#[from] InputSlotError),
    #[error("encountered an output slot error")]
    OutputSlotError(#[from] OutputSlotError),
    #[error("encountered an error when running a sub-graph")]
    RunSubGraphError(#[from] RunSubGraphError),
    #[error("encountered an error when executing draw command")]
    DrawError(#[from] DrawError),
}

/// A collection of input and output [`Edges`](Edge) for a [`Node`].
#[derive(Debug)]
pub struct Edges {
    label: InternedRenderLabel,
    input_edges: Vec<Edge>,
    output_edges: Vec<Edge>,
}

impl Edges {
    /// Returns all "input edges" (edges going "in") for this node .
    #[inline]
    pub fn input_edges(&self) -> &[Edge] {
        &self.input_edges
    }

    /// Returns all "output edges" (edges going "out") for this node .
    #[inline]
    pub fn output_edges(&self) -> &[Edge] {
        &self.output_edges
    }

    /// Returns this node's label.
    #[inline]
    pub fn label(&self) -> InternedRenderLabel {
        self.label
    }

    /// Adds an edge to the `input_edges` if it does not already exist.
    pub(crate) fn add_input_edge(&mut self, edge: Edge) -> Result<(), RenderGraphError> {
        if self.has_input_edge(&edge) {
            return Err(RenderGraphError::EdgeAlreadyExists(edge));
        }
        self.input_edges.push(edge);
        Ok(())
    }

    /// Removes an edge from the `input_edges` if it exists.
    pub(crate) fn remove_input_edge(&mut self, edge: Edge) -> Result<(), RenderGraphError> {
        if let Some(index) = self.input_edges.iter().position(|e| *e == edge) {
            self.input_edges.swap_remove(index);
            Ok(())
        } else {
            Err(RenderGraphError::EdgeDoesNotExist(edge))
        }
    }

    /// Adds an edge to the `output_edges` if it does not already exist.
    pub(crate) fn add_output_edge(&mut self, edge: Edge) -> Result<(), RenderGraphError> {
        if self.has_output_edge(&edge) {
            return Err(RenderGraphError::EdgeAlreadyExists(edge));
        }
        self.output_edges.push(edge);
        Ok(())
    }

    /// Removes an edge from the `output_edges` if it exists.
    pub(crate) fn remove_output_edge(&mut self, edge: Edge) -> Result<(), RenderGraphError> {
        if let Some(index) = self.output_edges.iter().position(|e| *e == edge) {
            self.output_edges.swap_remove(index);
            Ok(())
        } else {
            Err(RenderGraphError::EdgeDoesNotExist(edge))
        }
    }

    /// Checks whether the input edge already exists.
    pub fn has_input_edge(&self, edge: &Edge) -> bool {
        self.input_edges.contains(edge)
    }

    /// Checks whether the output edge already exists.
    pub fn has_output_edge(&self, edge: &Edge) -> bool {
        self.output_edges.contains(edge)
    }

    /// Searches the `input_edges` for a [`Edge::SlotEdge`],
    /// which `input_index` matches the `index`;
    pub fn get_input_slot_edge(&self, index: usize) -> Result<&Edge, RenderGraphError> {
        self.input_edges
            .iter()
            .find(|e| {
                if let Edge::SlotEdge { input_index, .. } = e {
                    *input_index == index
                } else {
                    false
                }
            })
            .ok_or(RenderGraphError::UnconnectedNodeInputSlot {
                input_slot: index,
                node: self.label,
            })
    }

    /// Searches the `output_edges` for a [`Edge::SlotEdge`],
    /// which `output_index` matches the `index`;
    pub fn get_output_slot_edge(&self, index: usize) -> Result<&Edge, RenderGraphError> {
        self.output_edges
            .iter()
            .find(|e| {
                if let Edge::SlotEdge { output_index, .. } = e {
                    *output_index == index
                } else {
                    false
                }
            })
            .ok_or(RenderGraphError::UnconnectedNodeOutputSlot {
                output_slot: index,
                node: self.label,
            })
    }
}

/// The internal representation of a [`Node`], with all data required
/// by the [`RenderGraph`](super::RenderGraph).
///
/// The `input_slots` and `output_slots` are provided by the `node`.
pub struct NodeState {
    pub label: InternedRenderLabel,
    /// The name of the type that implements [`Node`].
    pub type_name: &'static str,
    pub node: Box<dyn Node>,
    pub input_slots: SlotInfos,
    pub output_slots: SlotInfos,
    pub edges: Edges,
}

impl Debug for NodeState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "{:?} ({})", self.label, self.type_name)
    }
}

impl NodeState {
    /// Creates an [`NodeState`] without edges, but the `input_slots` and `output_slots`
    /// are provided by the `node`.
    pub fn new<T>(label: InternedRenderLabel, node: T) -> Self
    where
        T: Node,
    {
        NodeState {
            label,
            input_slots: node.input().into(),
            output_slots: node.output().into(),
            node: Box::new(node),
            type_name: core::any::type_name::<T>(),
            edges: Edges {
                label,
                input_edges: Vec::new(),
                output_edges: Vec::new(),
            },
        }
    }

    /// Retrieves the [`Node`].
    pub fn node<T>(&self) -> Result<&T, RenderGraphError>
    where
        T: Node,
    {
        self.node
            .downcast_ref::<T>()
            .ok_or(RenderGraphError::WrongNodeType)
    }

    /// Retrieves the [`Node`] mutably.
    pub fn node_mut<T>(&mut self) -> Result<&mut T, RenderGraphError>
    where
        T: Node,
    {
        self.node
            .downcast_mut::<T>()
            .ok_or(RenderGraphError::WrongNodeType)
    }

    /// Validates that each input slot corresponds to an input edge.
    pub fn validate_input_slots(&self) -> Result<(), RenderGraphError> {
        for i in 0..self.input_slots.len() {
            self.edges.get_input_slot_edge(i)?;
        }

        Ok(())
    }

    /// Validates that each output slot corresponds to an output edge.
    pub fn validate_output_slots(&self) -> Result<(), RenderGraphError> {
        for i in 0..self.output_slots.len() {
            self.edges.get_output_slot_edge(i)?;
        }

        Ok(())
    }
}

/// A [`Node`] without any inputs, outputs and subgraphs, which does nothing when run.
/// Used (as a label) to bundle multiple dependencies into one inside
/// the [`RenderGraph`](super::RenderGraph).
#[derive(Default)]
pub struct EmptyNode;

impl Node for EmptyNode {
    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        _render_context: &mut RenderContext,
        _world: &World,
    ) -> Result<(), NodeRunError> {
        Ok(())
    }
}

/// A [`RenderGraph`](super::RenderGraph) [`Node`] that runs the configured subgraph once.
/// This makes it easier to insert sub-graph runs into a graph.
pub struct RunGraphOnViewNode {
    sub_graph: InternedRenderSubGraph,
}

impl RunGraphOnViewNode {
    pub fn new<T: RenderSubGraph>(sub_graph: T) -> Self {
        Self {
            sub_graph: sub_graph.intern(),
        }
    }
}

impl Node for RunGraphOnViewNode {
    fn run(
        &self,
        graph: &mut RenderGraphContext,
        _render_context: &mut RenderContext,
        _world: &World,
    ) -> Result<(), NodeRunError> {
        graph.run_sub_graph(self.sub_graph, vec![], Some(graph.view_entity()))?;
        Ok(())
    }
}

/// This trait should be used instead of the [`Node`] trait when making a render node that runs on a view.
///
/// It is intended to be used with [`ViewNodeRunner`]
pub trait ViewNode {
    /// The query that will be used on the view entity.
    /// It is guaranteed to run on the view entity, so there's no need for a filter
    type ViewQuery: ReadOnlyQueryData;

    /// Updates internal node state using the current render [`World`] prior to the run method.
    fn update(&mut self, _world: &mut World) {}

    /// Runs the graph node logic, issues draw calls, updates the output slots and
    /// optionally queues up subgraphs for execution. The graph data, input and output values are
    /// passed via the [`RenderGraphContext`].
    fn run<'w>(
        &self,
        graph: &mut RenderGraphContext,
        render_context: &mut RenderContext<'w>,
        view_query: QueryItem<'w, '_, Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError>;
}

/// This [`Node`] can be used to run any [`ViewNode`].
/// It will take care of updating the view query in `update()` and running the query in `run()`.
///
/// This [`Node`] exists to help reduce boilerplate when making a render node that runs on a view.
pub struct ViewNodeRunner<N: ViewNode> {
    view_query: QueryState<N::ViewQuery>,
    node: N,
}

impl<N: ViewNode> ViewNodeRunner<N> {
    pub fn new(node: N, world: &mut World) -> Self {
        Self {
            view_query: world.query_filtered(),
            node,
        }
    }
}

impl<N: ViewNode + FromWorld> FromWorld for ViewNodeRunner<N> {
    fn from_world(world: &mut World) -> Self {
        Self::new(N::from_world(world), world)
    }
}

impl<T> Node for ViewNodeRunner<T>
where
    T: ViewNode + Send + Sync + 'static,
{
    fn update(&mut self, world: &mut World) {
        self.view_query.update_archetypes(world);
        self.node.update(world);
    }

    fn run<'w>(
        &self,
        graph: &mut RenderGraphContext,
        render_context: &mut RenderContext<'w>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        let Ok(view) = self.view_query.get_manual(world, graph.view_entity()) else {
            return Ok(());
        };

        ViewNode::run(&self.node, graph, render_context, view, world)?;
        Ok(())
    }
}
