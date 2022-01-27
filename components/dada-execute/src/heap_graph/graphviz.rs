use dada_collections::{IndexSet, Map};
use dada_id::InternKey;
use dada_parse::prelude::*;

use super::{DataNode, HeapGraph, PermissionNode, ValueEdge, ValueEdgeTarget};

impl HeapGraph {
    /// Plots this heap-graph by itself.
    pub fn graphviz_alone(&self, db: &dyn crate::Db, include_temporaries: bool) -> String {
        let mut output = vec![];
        let mut writer = GraphvizWriter {
            db,
            name_prefix: "",
            writer: &mut std::io::Cursor::new(&mut output),
            indent: 0,
            include_temporaries,
            node_queue: Default::default(),
            node_set: Default::default(),
            permissions: Default::default(),
            value_edge_list: vec![],
        };
        self.to_graphviz(&mut writer, |w| self.stack_and_heap(w))
            .unwrap();
        String::from_utf8(output).unwrap()
    }

    /// Plots this heap-graph as the "state at start of breakpoint", with `heap_graph_end` as "state at end of breakpoint".
    pub fn graphviz_paired(
        &self,
        db: &dyn crate::Db,
        include_temporaries: bool,
        heap_graph_end: &HeapGraph,
    ) -> String {
        let mut output = vec![];
        let mut writer = GraphvizWriter {
            db,
            name_prefix: "",
            writer: &mut std::io::Cursor::new(&mut output),
            indent: 0,
            include_temporaries,
            node_queue: Default::default(),
            node_set: Default::default(),
            permissions: Default::default(),
            value_edge_list: vec![],
        };
        self.to_graphviz(&mut writer, |w| {
            w.name_prefix("after");
            w.indent("subgraph cluster_after {")?;
            w.println("label=<<b>after</b>>")?;
            heap_graph_end.stack_and_heap(w)?;
            w.undent("}")?;

            w.name_prefix("before");
            w.indent("subgraph cluster_before {")?;
            w.println("label=<<b>before</b>>")?;
            self.stack_and_heap(w)?;
            w.undent("}")?;

            Ok(())
        })
        .unwrap();
        String::from_utf8(output).unwrap()
    }

    /*
        digraph G {
        node[shape="rectangle"];

        rankdir = "LR";

        subgraph cluster_stack {
            label=<<b>stack</b>>;
            rank="source";
            stack[shape="none", label=<
                <table border="0">
                <tr><td border="1">foo()</td></tr>
                <tr><td port="0">p</td></tr>
                <tr><td port="1">q</td></tr>
                </table>
            >];
        }

        object[shape="note", label=<
            <table border="0">
            <tr><td border="1">Point</td></tr>
            <tr><td port="0">x</td></tr>
            <tr><td port="1">y</td></tr>
            </table>
        >];

        stack:0 -> object [label="my"];
        stack:1 -> object [label="leased(p)"];
    }
     */

    fn to_graphviz(
        &self,
        w: &mut GraphvizWriter<'_>,
        contents: impl FnOnce(&mut GraphvizWriter<'_>) -> eyre::Result<()>,
    ) -> eyre::Result<()> {
        w.indent("digraph {")?;
        w.println(r#"node[shape = "note"];"#)?;
        w.println(r#"rankdir = "LR";"#)?;

        contents(w)?;

        w.undent("}")?;

        Ok(())
    }

    fn stack_and_heap(&self, w: &mut GraphvizWriter<'_>) -> eyre::Result<()> {
        self.print_stack(w)?;

        self.print_heap(w)?;

        let value_edge_list = std::mem::take(&mut w.value_edge_list);
        for value_edge in &value_edge_list {
            let permission_data = value_edge.permission.data(&self.tables);
            let label = permission_data.label.as_str();

            let style = if permission_data.tenant.is_some() {
                "dotted"
            } else {
                "solid"
            };

            w.println(format!(
                r#"{source:?}:{source_port} -> {target:?} [label={label:?}, style={style:?}];"#,
                source = value_edge.source.node,
                source_port = value_edge.source.port,
                target = value_edge.target,
            ))?;
        }

        Ok(())
    }

    fn find_lessor_place<'w>(
        &self,
        w: &'w GraphvizWriter<'_>,
        permission: PermissionNode,
    ) -> Vec<GraphvizPlace> {
        if let Some(place) = w.permissions.get(&permission) {
            return place.clone();
        }

        if let Some(lessor) = permission.data(&self.tables).lessor {
            return self.find_lessor_place(w, lessor);
        }

        vec![]
    }

    fn print_stack(&self, w: &mut GraphvizWriter<'_>) -> eyre::Result<()> {
        let np = w.name_prefix;

        w.indent(format!("subgraph cluster_{np}stack {{"))?;
        w.println("label=<<b>stack</b>>")?;
        w.println(r#"rank="source";"#)?;

        let stack_node_name = format!("{np}stack");
        w.indent(format!(r#"{stack_node_name}["#))?;
        w.println(r#"shape="none";"#)?;
        w.indent(r#"label=<"#)?;
        w.println(r#"<table border="0">"#)?;
        let mut field_index = 0;
        for stack_frame_node in &self.stack {
            let stack_frame_data = stack_frame_node.data(&self.tables);
            let function_name = stack_frame_data.function.name(w.db).as_str(w.db);
            w.println(format!(r#"<tr><td border="1">{function_name}</td></tr>"#))?;

            let include_temporaries = w.include_temporaries;
            let names = stack_frame_data.variables.iter().map(|v| match v.name {
                Some(word) => Some(word.as_str(w.db).to_string()),
                None if include_temporaries => Some(format!("{:?}", v.id)),
                None => None,
            });

            field_index = self.print_fields(
                w,
                &stack_node_name,
                names,
                stack_frame_data.variables.iter().map(|v| &v.value),
                field_index,
            )?;

            if let Some(in_flight_value) = &stack_frame_data.in_flight_value {
                self.print_field(
                    w,
                    in_flight_value,
                    Some(&"(in-flight)".to_string()),
                    "stack",
                    field_index,
                )?;
                field_index += 1;
            }
        }
        w.println(r#"</table>"#)?;
        w.undent(r#">;"#)?;
        w.undent(r#"];"#)?;
        w.undent("}")?;
        Ok(())
    }

    fn print_heap(&self, w: &mut GraphvizWriter<'_>) -> eyre::Result<()> {
        while let Some(edge) = w.node_queue.pop() {
            self.print_heap_node(w, edge)?;
        }
        Ok(())
    }

    fn print_heap_node(
        &self,
        w: &mut GraphvizWriter<'_>,
        edge: ValueEdgeTarget,
    ) -> eyre::Result<()> {
        let name = w.node_name(&edge);
        w.indent(format!(r#"{name} ["#))?;
        match edge {
            ValueEdgeTarget::Object(o) => {
                let data = o.data(&self.tables);
                let fields = data.class.fields(w.db);
                let field_names = fields
                    .iter()
                    .map(|f| Some(f.name(w.db).as_str(w.db).to_string()));
                w.indent(r#"label = <<table border="0">"#)?;
                let class_name = data.class.name(w.db).as_str(w.db);
                w.println(format!(r#"<tr><td border="1">{class_name}</td></tr>"#))?;
                self.print_fields(w, &name, field_names, &data.fields, 0)?;
                w.undent(r#"</table>>"#)?;
            }
            ValueEdgeTarget::Class(c) => {
                let name = c.name(w.db).as_str(w.db);
                w.println(format!(r#"label = <<b>{name}</b>>"#))?;
            }
            ValueEdgeTarget::Function(f) => {
                let name = f.name(w.db).as_str(w.db);
                w.println(format!(r#"label = <<b>{name}()</b>>"#))?;
            }
            ValueEdgeTarget::Data(d) => {
                let data_str = self.data_str(d);
                w.println(format!(r#"label = {data_str:?}"#))?;
            }
        }
        w.undent(r#"];"#)?;
        Ok(())
    }

    fn print_fields<'me>(
        &self,
        w: &mut GraphvizWriter,
        source: &str,
        names: impl IntoIterator<Item = Option<String>>,
        edges: impl IntoIterator<Item = &'me ValueEdge>,
        mut index: usize,
    ) -> eyre::Result<usize> {
        for (edge, name) in edges.into_iter().zip(names) {
            self.print_field(w, edge, name.as_ref(), source, index)?;
            index += 1;
        }
        Ok(index)
    }

    fn print_field(
        &self,
        w: &mut GraphvizWriter,
        edge: &ValueEdge,
        name: Option<&String>,
        source: &str,
        index: usize,
    ) -> Result<(), eyre::Error> {
        let edge: &ValueEdge = edge;
        if let Some(name) = name {
            w.permissions
                .entry(edge.permission)
                .or_insert(vec![])
                .push(GraphvizPlace {
                    node: source.to_string(),
                    port: index,
                });

            if let ValueEdgeTarget::Data(d) = edge.target {
                let data_str = self.data_str(d);
                w.println(format!(
                    r#"<tr><td port="{index}">{name}: {data_str}</td></tr>"#,
                ))?;
            } else {
                w.println(format!(r#"<tr><td port="{index}">{name}</td></tr>"#))?;
                w.push_value_edge(source, index, edge, edge.permission);
            }
        }
        Ok(())
    }

    fn data_str(&self, d: DataNode) -> String {
        let data_str = format!("{:?}", d.data(&self.tables).debug);
        let data = html_escape::encode_text(&data_str).to_string();
        if data.len() < 40 {
            data
        } else {
            let len = data.len() - 20;
            format!("{}[...]{}", &data[0..20], &data[len..])
        }
    }
}

struct GraphvizWriter<'w> {
    /// If true, include temporary variables from stack frames
    /// in the output (usually false).
    include_temporaries: bool,

    /// Queue of edges to process.
    node_queue: Vec<ValueEdgeTarget>,

    /// Set of all edges we have ever processed; when a new edge
    /// is added to this set, it is pushed to the queue.
    node_set: IndexSet<ValueEdgeTarget>,

    /// A collection of edges from fields to their values,
    /// accumulated as we walk the `HeapGraph` and then
    /// dumped out at the end.
    value_edge_list: Vec<GraphvizValueEdge>,

    /// Maps from each permission to the place whose value has it.
    permissions: Map<PermissionNode, Vec<GraphvizPlace>>,

    /// The crate database.
    db: &'w dyn crate::Db,

    /// Where we write the output.
    writer: &'w mut dyn std::io::Write,

    /// Current indentation in spaces.
    indent: usize,

    /// String to prefix on all node names.
    name_prefix: &'static str,
}

/// Identifies a particular "place" in the graphviz output;
/// this is either a field or a local variable.
#[derive(Clone, Debug)]
struct GraphvizPlace {
    /// Id of the node within graphviz.
    node: String,

    /// Port of the row for this field.
    port: usize,
}

struct GraphvizValueEdge {
    source: GraphvizPlace,
    target: String,
    permission: PermissionNode,
}

impl GraphvizWriter<'_> {
    fn name_prefix(&mut self, prefix: &'static str) {
        self.name_prefix = prefix;
    }

    fn indent(&mut self, s: impl AsRef<str>) -> eyre::Result<()> {
        self.println(s)?;
        self.indent += 2;
        Ok(())
    }

    fn println(&mut self, s: impl AsRef<str>) -> eyre::Result<()> {
        let s = s.as_ref();
        writeln!(
            self.writer,
            "{space:indent$}{s}",
            space = "",
            indent = self.indent,
            s = s
        )?;
        Ok(())
    }

    fn undent(&mut self, s: impl AsRef<str>) -> eyre::Result<()> {
        self.indent -= 2;
        self.println(s)
    }

    fn push_value_edge(
        &mut self,
        source: &str,
        source_port: usize,
        edge: &ValueEdge,
        permission: PermissionNode,
    ) {
        let name = self.node_name(&edge.target);
        self.value_edge_list.push(GraphvizValueEdge {
            source: GraphvizPlace {
                node: source.to_string(),
                port: source_port,
            },
            target: name,
            permission,
        });
    }

    fn node_name(&mut self, edge: &ValueEdgeTarget) -> String {
        let (index, new) = self.node_set.insert_full(*edge);
        if new {
            self.node_queue.push(*edge);
        }
        let np = self.name_prefix;
        format!("{np}node{index}")
    }
}