
// Makes a list of "item" fields
function sepSeq1(rule, sepChar) {
    const itemRule = field("item", rule);
    return seq(itemRule, repeat(seq(sepChar, itemRule)))
}

// Makes a list of "item" fields
function sepSeq(rule, sepChar) {
    return optional(sepSeq1(rule, sepChar))
}

// Makes a list of "item" fields
function newlineSepSeq($, rule) {
    return seq(
        optional($._linebreak),
        optional(seq(
            field('item', rule),
            repeat(seq(
                $._linebreak,
                field('item', rule)
            )),
            optional($._linebreak)
        ))
    )
}

// Makes a list of "item" fields
function commaSepSeq($, rule) {
    return seq(
        optional($._linebreak),
        optional(seq(
            field('item', rule),
            repeat(seq(
                $._comma,
                field('item', rule)
            )),
            optional($._linebreak)
        ))
    )
}

const PREC = {
    part_select: 2,
    compare : 3,
    xor: 4,
    or: 5,
    and: 6,
    additive: 7,
    multiplicative: 8,
    unary: 9,
    postscript_op : 10
}

module.exports = grammar({
    name: 'sus',

    rules: {
        // Top level structure

        source_file: $ => newlineSepSeq($, $.global_object),

        global_object: $ => seq(
            optional(field('extern_marker', choice('__builtin__', 'extern'))),
            // Because we want to reuse our "generative code", we parse them under the same umbrella. 
            // Their differences are their semantic meaning, and therefore what constructs are allowed in each
            // For instance, modules have no restrictions
            // Consts only contain generative code (with generative parameters they're similar to functions)
            // Struct defines types, and cannot contain non-generative operations. (Only non-generative declarations are allowed, these define the fields)
            field('object_type', choice('module', 'struct', $.const_and_type)),
            field('name', $.identifier),
            optional(field('template_declaration_arguments', $.template_declaration_arguments)),
            field('block', $.block)
        ),

        const_and_type: $ => seq(
            'const',
            field('const_type', $._type)
        ),

        // Template Declaration

        template_declaration_arguments: $ => seq(
            '#(',
            commaSepSeq($, choice(
                $.template_declaration_type,
                $.declaration
            )),
            ')'
        ),

        template_declaration_type: $ => seq(
            field('name', $.identifier)
        ),

        // Statements

        block: $ => seq(
            '{',
            newlineSepSeq($, choice(
                $.block,
                $.decl_assign_statement,

                // Decls should only allow a single declaration, and cannot contain expressions,
                // but we allow some tolerance in the grammar here, so we can generate better errors after.
                $.assign_left_side,
                $.if_statement,
                $.for_statement,
                $.domain_statement,
                $.interface_statement
            )),
            '}'
        ),

        decl_assign_statement: $ => seq(
            field('assign_left', $.assign_left_side),
            '=',
            field('assign_value', $._expression)
        ),
        assign_left_side: $ => sepSeq1($.assign_to, $._comma),
        assign_to: $ => seq(
            optional(field('write_modifiers', $.write_modifiers)),
            field('expr_or_decl', choice(
                $.declaration,
                $._expression
            ))
        ),
        write_modifiers: $ => choice(
            repeat1(field('item', 'reg')),
            field('item', 'initial')
        ),

        _then_else_block: $ => seq(
            field('then_block', $.block),
            optional(field('else_block', $.else_block))
        ),

        if_statement: $ => seq(
            field('statement_type',choice(
                'when',
                'if'
            )),
            field('condition', $._expression),
            optional(field('conditional_bindings', $.interface_ports)),
            $._then_else_block
        ),
        else_block: $ => seq(
            'else',
            field('content', choice(
                $.block,
                $.if_statement
            ))
        ),
        for_statement: $ => seq(
            field('for_kw', 'for'),
            field('for_decl', $.declaration),
            'in',
            field('from', $._expression),
            '..',
            field('to', $._expression),
            field('block', $.block)
        ),

        // Interfaces

        domain_statement: $ => seq(
            'domain',
            field('name', $.identifier),
        ),

        interface_statement: $ => seq(
            optional(field('local', 'local')),
            field('interface_kind', choice('interface', 'action', 'trigger')),
            field('name', $.identifier),
            optional(field('latency_specifier', $.latency_specifier)),
            optional(field('interface_ports', $.interface_ports)),
            optional($._then_else_block)
        ),

        interface_ports: $ => seq(
            ':',
            optional($._linebreak),
            choice(
                seq(
                    field('inputs', $.declaration_list),
                    optional($._interface_ports_output)
                ),
                $._interface_ports_output
            ),
        ),
        _interface_ports_output: $ => seq(
            '->',
            optional($._linebreak),
            field('outputs', $.declaration_list)
        ),

        // Declarations

        declaration_list: $ => sepSeq1($.declaration, $._comma),

        declaration: $ => seq(
            optional(field('declaration_modifiers', $.declaration_modifiers)),
            field('type', $._type),
            field('name', $.identifier),
            optional(field('latency_specifier', $.latency_specifier))
        ),
        declaration_modifiers: $ => repeat1(field('item', choice(
            'state',
            'gen',
            'input',
            'output'
        ))),

        latency_specifier: $ => seq(
            '\'',
            field('content', $._expression)
        ),

        // Types

        _type: $ => choice(
            $.template_global,
            $.array_type
        ),

        array_type: $ => seq(
            field('arr', $._type),
            field('arr_idx', $.array_type_bracket)
        ),

        // Expressions

        _expression: $ => choice(
            $.template_global,
            $.array_op,
            $.number,
            $.parenthesis_expression,
            $.unary_op,
            $.binary_op,
            $.func_call,
            $.field_access,
            $.array_list_expression
        ),

        unary_op: $ => prec(PREC.unary, seq(
            field('operator', choice('+', '-', '*', '!', '|', '&', '^')),
            field('right', $._expression)
        )),

        binary_op: $ => {
            const TABLE = [
                [PREC.compare, choice('==', '!=', '<', '<=', '>', '>=')],
                [PREC.xor, '^'],
                [PREC.or, '|'],
                [PREC.and, '&'],
                [PREC.additive, choice('+', '-')],
                [PREC.multiplicative, choice('*', '/', '%')],
            ];

            return choice(...TABLE.map(([precedence, operator]) => prec.left(precedence, seq(
                field('left', $._expression),
                field('operator', operator),
                field('right', $._expression)
            ))));
        },

        array_op: $ => prec(PREC.postscript_op, seq(
            field('arr', $._expression),
            field('arr_idx', $.array_access_bracket_expression)
        )),

        func_call: $ => prec(PREC.postscript_op, seq(
            field('name', $._expression),
            field('arguments', $.parenthesis_expression_list)
        )),

        field_access: $ => prec(PREC.postscript_op, seq(
            field('left', $._expression),
            '.',
            field('name', $.identifier)
        )),

        parenthesis_expression_list: $ => seq(
            '(',
            sepSeq($._expression, $._comma),
            ')'
        ),
        

        parenthesis_expression: $ => seq(
            '(',
            field('content', $._expression),
            ')'
        ),

        array_type_bracket: $ => seq(
            '[',
            field('content', $._expression),
            ']',
        ),

        array_access_bracket_expression: $ => seq(
            '[',
            choice(
                field('slice', $.slice),
                field('index', $._expression)
            ),
            ']'
        ),
        
        slice: $ => seq(
            optional(field('index_a', $._expression)),
            prec(PREC.part_select, field('type', choice(':', '+:', '-:'))),
            optional(field('index_b', $._expression))
        ),

        array_list_expression: $ => seq(
            '[',
            commaSepSeq($, $._expression),
            ']'
        ),

        // Utilities

        namespace_list: $ => sepSeq1($.identifier, '::'),

        // myFunc #(T: type int, VAL: 2)
        template_global: $ => seq(
            optional(field('is_global_path', '::')),
            field('namespace_list', $.namespace_list),

            // Template
            optional(field('template_args', $.template_args))
        ),

        template_args: $ => seq(
            '#(',
            commaSepSeq($, $.template_arg),
            ')'
        ),

        template_arg : $ => seq(
            field('name', $.identifier),
            optional(seq(
                ':',
                choice(
                    seq('type', field('type_arg', $._type)),
                    field('val_arg', $._expression)
                )
            )),
        ),

        identifier: $ => /[\p{Alphabetic}_][\p{Alphabetic}_\p{Decimal_Number}]*/,
        number: $ => /\d[\d_]*/,

        _comma: $ => seq(
            ',',
            optional($._linebreak)
        ),

        _linebreak: $ => repeat1('\n'), // For things that must be separated by at least one newline (whitespace after is to optimize gobbling up any extra newlines)

        // Extras
        
        // These must be kept in this order for precedence, so that
        // '///' is not interpreted as a single_line_comment of '/'
        doc_comment: $ => /\/\/\/[^\n]*/,
        single_line_comment: $ => /\/\/[^\n]*/,

        multi_line_comment: $ => /\/\*[^\*]*\*+([^\/\*][^\*]*\*+)*\//,
    },

    conflicts: $ => [
        [$._type, $._expression], // Just because LR(1) is too weak to resolve 'ident[] a' vs 'type_name[]'. Tree sitter resolves this itself with more expensive GLR. NOT a precedence relation. 
    ],

    word: $=> $.identifier,

    extras: $ => [
        /[ \t\r]+/, // Non newline whitespace
        $.doc_comment,
        $.single_line_comment,
        $.multi_line_comment
    ]
});

