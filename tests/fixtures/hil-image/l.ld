ENTRY(_start)
PHDRS { pa PT_LOAD; pb PT_LOAD; }
SECTIONS { . = 0x20000; .a : { *(.text.a) } :pa . = 0x20400; .b : { *(.text.b) } :pb }
