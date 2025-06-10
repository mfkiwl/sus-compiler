use sus_proc_macro::{field, kind, kw};

use crate::errors::ErrorStore;
use crate::linker::{IsExtern, AFTER_INITIAL_PARSE_CP};
use crate::prelude::*;

use crate::flattening::Module;
use crate::linker::{FileBuilder, LinkInfo, ResolvedGlobals};

use super::parser::Cursor;
use super::*;

struct InitializationContext<'linker> {
    // module-only stuff
    ports: FlatAlloc<Port, PortIDMarker>,
    interfaces: FlatAlloc<Interface, InterfaceIDMarker>,
    domains: FlatAlloc<DomainInfo, DomainIDMarker>,
    /// This is initially true, but when the first `domain xyz` statement is encountered this is set to false.
    implicit_clk_domain: bool,

    // struct-only stuff
    fields: FlatAlloc<StructField, FieldIDMarker>,

    errors: ErrorCollector<'linker>,
}

impl InitializationContext<'_> {
    fn gather_initial_global_object(&mut self, cursor: &mut Cursor) -> (Span, String) {
        let (name_span, name) = cursor.field_to_string(field!("name"), kind!("identifier"));
        self.domains.alloc(DomainInfo {
            name: "clk".to_string(),
            name_span: None,
        });
        let _ = cursor.optional_field(field!("template_declaration_arguments"));

        cursor.field(field!("block"));
        self.gather_all_ports_in_block(cursor);

        (name_span, name)
    }

    fn gather_ports_in_if_stmt(&mut self, cursor: &mut Cursor) {
        cursor.go_down_no_check(|cursor| {
            cursor.field(field!("statement_type"));
            cursor.field(field!("condition"));
            cursor.field(field!("then_block"));
            self.gather_all_ports_in_block_and_else_block(cursor);
        })
    }

    fn gather_assign_left_side(&mut self, cursor: &mut Cursor) {
        cursor.list(kind!("assign_left_side"), |cursor| {
            cursor.go_down(kind!("assign_to"), |cursor| {
                let _ = cursor.optional_field(field!("write_modifiers"));
                cursor.field(field!("expr_or_decl"));

                if cursor.kind() == kind!("declaration") {
                    let whole_decl_span = cursor.span();
                    cursor.go_down_no_check(|cursor| {
                        self.gather_decl(None, whole_decl_span, cursor);
                    });
                }
            });
        });
    }

    fn gather_all_ports_in_block(&mut self, cursor: &mut Cursor) {
        cursor.list(kind!("block"), |cursor| {
            match cursor.kind() {
                kind!("domain_statement") => {
                    let whole_domain_statement_span = cursor.span();
                    cursor.go_down_no_check(|cursor| {
                        let (domain_name_span, domain_name) =
                            cursor.field_to_string(field!("name"), kind!("identifier"));
                        if self.implicit_clk_domain {
                            if let Some(existing_port) = self.ports.iter().next() {
                                // Sad Path: Having ports on the implicit clk domain is not allowed. 
                                self.errors.error(whole_domain_statement_span, "When using explicit domains, no port is allowed to be declared on the implicit 'clk' domain.")
                                    .info_same_file(existing_port.1.decl_span, "A domain should be explicitly defined before this port");
                            } else {
                                // Happy Path: The implicit clk domain hasn't been used yet, so we can safely get rid of it. 
                                self.domains.clear();
                            }

                            self.implicit_clk_domain = false;
                        }
                        self.domains.alloc(DomainInfo { name: domain_name, name_span: Some(domain_name_span) })
                    });
                }
                kind!("interface_statement") => {
                    cursor.go_down_no_check(|cursor| {
                        let (name_span, name) = cursor.field_to_string(field!("name"), kind!("identifier"));

                        self.gather_func_call_ports(name_span, name, cursor);
                    });
                }
                kind!("block") => {
                    self.gather_all_ports_in_block(cursor);
                }
                kind!("if_statement") => {
                    self.gather_ports_in_if_stmt(cursor);
                }
                kind!("for_statement") => cursor.go_down_no_check(|cursor| {
                    cursor.field(field!("for_decl"));
                    cursor.field(field!("from"));
                    cursor.field(field!("to"));
                    cursor.field(field!("block"));
                    self.gather_all_ports_in_block(cursor);
                }),
                kind!("assign_left_side") => {
                    self.gather_assign_left_side(cursor);
                }
                kind!("decl_assign_statement") => {
                    cursor.go_down_no_check(|cursor| {
                        cursor.field(field!("assign_left"));
                        self.gather_assign_left_side(cursor);
                    });
                }
                _other => {} // Nothing
            }
        });
    }

    fn gather_all_ports_in_block_and_else_block(&mut self, cursor: &mut Cursor) {
        self.gather_all_ports_in_block(cursor);
        if cursor.optional_field(field!("else_block")) {
            cursor.go_down_no_check(|cursor| {
                cursor.field(field!("content"));
                match cursor.kind() {
                    kind!("if_statement") => {
                        self.gather_ports_in_if_stmt(cursor);
                    }
                    kind!("block") => {
                        self.gather_all_ports_in_block(cursor);
                    }
                    _other => unreachable!(),
                }
            });
        }
    }

    fn gather_func_call_ports(
        &mut self,
        interface_name_span: Span,
        name: String,
        cursor: &mut Cursor,
    ) {
        let ports = if cursor.optional_field(field!("interface_ports")) {
            cursor.go_down(kind!("interface_ports"), |cursor| {
                let inputs = cursor
                    .optional_field(field!("inputs"))
                    .then(|| self.gather_decl_names_in_list(true, cursor));
                let outputs = cursor
                    .optional_field(field!("outputs"))
                    .then(|| self.gather_decl_names_in_list(false, cursor));
                (inputs, outputs)
            })
        } else {
            (None, None)
        };

        let (func_call_inputs, func_call_outputs) = match ports {
            (None, None) => (PortIDRange::empty(), PortIDRange::empty()),
            (None, Some(fouts)) => (PortIDRange::new(fouts.0, fouts.0), fouts),
            (Some(fins), None) => (fins, PortIDRange::new(fins.1, fins.1)),
            (Some(fins), Some(fouts)) => (fins, fouts),
        };
        // All ports are consecutive
        assert_eq!(func_call_inputs.1, func_call_outputs.0);

        self.interfaces.alloc(Interface {
            func_call_inputs,
            func_call_outputs,
            domain: self.domains.last_id(),
            name_span: interface_name_span,
            name,
        });
    }

    fn gather_decl_names_in_list(&mut self, is_input: bool, cursor: &mut Cursor) -> PortIDRange {
        let list_start_at = self.ports.get_next_alloc_id();
        cursor.list(kind!("declaration_list"), |cursor| {
            let whole_decl_span = cursor.span();
            cursor.go_down(kind!("declaration"), |cursor| {
                self.gather_decl(Some(is_input), whole_decl_span, cursor);
            });
        });
        self.ports.range_since(list_start_at)
    }

    fn gather_decl(
        &mut self,
        mut is_input: Option<bool>,
        whole_decl_span: Span,
        cursor: &mut Cursor,
    ) {
        // If generative input it's a template arg
        let mut is_generative = false;
        if cursor.optional_field(field!("declaration_modifiers")) {
            cursor.list(kind!("declaration_modifiers"), |cursor| {
                match cursor.kind() {
                    kw!("gen") => {
                        is_generative = true;
                    }
                    kw!("state") => {}
                    kw!("input") => {
                        is_input = Some(true);
                    }
                    kw!("output") => {
                        is_input = Some(false);
                    }
                    _ => {}
                }
            });
        }

        if is_generative {
            is_input = None;
        }

        cursor.field(field!("type"));
        let type_span = cursor.span();
        let decl_span = Span::new_overarching(type_span, whole_decl_span.empty_span_at_end());
        let (name_span, name) = cursor.field_to_string(field!("name"), kind!("identifier"));

        match (is_generative, is_input) {
            (false, Some(is_input)) => {
                self.ports.alloc(Port {
                    name,
                    name_span,
                    decl_span,
                    is_input,
                    domain: self.domains.last_id(),
                    declaration_instruction: FlatID::PLACEHOLDER,
                });
            }
            (false, None) => {
                self.fields.alloc(StructField {
                    name,
                    name_span,
                    decl_span,
                    declaration_instruction: FlatID::PLACEHOLDER,
                });
            }
            _other => {}
        }
    }
}

pub fn gather_initial_file_data(mut builder: FileBuilder) {
    assert!(builder.file_data.associated_values.is_empty());

    let mut cursor = match Cursor::new_at_root(builder.file_data) {
        Ok(cursor) => cursor,
        Err(file_span) => {
            builder
                .other_parsing_errors
                .error(file_span, "An ERROR node at the root of the syntax tree!");

            return;
        }
    };

    cursor.list_and_report_errors(
        kind!("source_file"),
        builder.other_parsing_errors,
        |cursor| {
            let parsing_errors = ErrorCollector::new_empty(builder.file_id, builder.files);
            cursor.report_all_decendant_errors(&parsing_errors);

            let span = cursor.span();
            cursor.go_down(kind!("global_object"), |cursor| {
                initialize_global_object(&mut builder, parsing_errors, span, cursor);
            });
        },
    );
}

enum GlobalObjectKind {
    Module,
    Const,
    Struct,
}

fn initialize_global_object(
    builder: &mut FileBuilder,
    parsing_errors: ErrorCollector,
    span: Span,
    cursor: &mut Cursor,
) {
    let is_extern = match cursor
        .optional_field(field!("extern_marker"))
        .then(|| cursor.kind())
    {
        None => IsExtern::Normal,
        Some(kw!("extern")) => IsExtern::Extern,
        Some(kw!("__builtin__")) => IsExtern::Builtin,
        Some(_) => cursor.could_not_match(),
    };

    cursor.field(field!("object_type"));
    let global_obj_kind = match cursor.kind() {
        kw!("module") => GlobalObjectKind::Module,
        kind!("const_and_type") => GlobalObjectKind::Const,
        kw!("struct") => GlobalObjectKind::Struct,
        _other => cursor.could_not_match(),
    };

    let mut ctx = InitializationContext {
        ports: FlatAlloc::new(),
        interfaces: FlatAlloc::new(),
        domains: FlatAlloc::new(),
        implicit_clk_domain: true,
        fields: FlatAlloc::new(),
        errors: parsing_errors,
    };

    let (name_span, name) = ctx.gather_initial_global_object(cursor);

    let mut link_info = LinkInfo {
        template_parameters: FlatAlloc::new(),
        instructions: FlatAlloc::new(),
        documentation: cursor.extract_gathered_comments(),
        file: builder.file_id,
        name,
        name_span,
        span,
        errors: ErrorStore::new(),
        is_extern,
        resolved_globals: ResolvedGlobals::empty(),
        checkpoints: Vec::new(),
    };

    link_info.reabsorb_errors(ctx.errors.into_storage());
    link_info.checkpoint(AFTER_INITIAL_PARSE_CP);

    match global_obj_kind {
        GlobalObjectKind::Module => {
            builder.add_module(Module {
                link_info,
                ports: ctx.ports,
                latency_inference_info: PortLatencyInferenceInfo::default(),
                domains: ctx.domains,
                interfaces: ctx.interfaces,
            });
        }
        GlobalObjectKind::Struct => {
            builder.add_type(StructType {
                link_info,
                fields: ctx.fields,
            });
        }
        GlobalObjectKind::Const => {
            builder.add_const(NamedConstant {
                link_info,
                output_decl: FlatID::PLACEHOLDER,
            });
        }
    }
}
