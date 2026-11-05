/* CI-only driver: stdout is a complete AST (including extras/anonymous nodes),
 * stderr is scanner instrumentation. Never linked into the product. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <tree_sitter/api.h>

extern const TSLanguage *tree_sitter_python(void);
extern void probe_report(void);

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    FILE *f = fopen(argv[1], "rb");
    if (!f) return 2;
    fseek(f, 0, SEEK_END);
    long length = ftell(f);
    rewind(f);
    char *source = malloc((size_t)length + 1);
    if (!source || fread(source, 1, length, f) != (size_t)length) return 2;
    fclose(f);
    TSParser *parser = ts_parser_new();
    if (!ts_parser_set_language(parser, tree_sitter_python())) return 2;
    struct timespec start, end;
    clock_gettime(CLOCK_MONOTONIC, &start);
    TSTree *tree = ts_parser_parse_string(parser, NULL, source, length);
    clock_gettime(CLOCK_MONOTONIC, &end);
    if (!tree) return 3;
    fprintf(stderr, "{\"seconds\":%.9f,\"states\":[", (end.tv_sec-start.tv_sec) +
            (end.tv_nsec-start.tv_nsec)/1e9);
    probe_report();
    fprintf(stderr, "]}\n");
    TSNode root = ts_tree_root_node(tree);
    char *sexp = ts_node_string(root);
    puts(sexp);
    free(sexp);
    TSTreeCursor cursor = ts_tree_cursor_new(root);
    unsigned depth = 0;
    for (;;) {
        TSNode node = ts_tree_cursor_current_node(&cursor);
        printf("%u %s %u %u %d %d\n", depth, ts_node_type(node),
               ts_node_start_byte(node), ts_node_end_byte(node),
               ts_node_is_named(node), ts_node_is_extra(node));
        if (ts_tree_cursor_goto_first_child(&cursor)) { depth++; continue; }
        while (!ts_tree_cursor_goto_next_sibling(&cursor)) {
            if (!ts_tree_cursor_goto_parent(&cursor)) goto done;
            depth--;
        }
    }
done:
    ts_tree_cursor_delete(&cursor);
    ts_tree_delete(tree);
    ts_parser_delete(parser);
    free(source);
    return 0;
}
