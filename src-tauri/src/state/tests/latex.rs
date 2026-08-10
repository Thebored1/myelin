use super::*;

    #[test]
    fn bare_latex_keeps_partial_preamble_before_document_body() {
        let transform = wrap_bare_latex("\\usepackage[dvipsnames]{xcolor}\n\n\\textcolor{Red}{Body}");
        assert!(transform.source.contains("\\usepackage[dvipsnames]{xcolor}\n\n\\begin{document}"));
        let body_line = transform
            .source
            .lines()
            .position(|line| line.contains("textcolor"))
            .unwrap();
        assert_eq!(transform.line_map[body_line], 3);
    }

    #[test]
    fn package_detection_ignores_comments_and_handles_options_and_requirements() {
        let source = concat!(
            "\\documentclass{article}\n",
            "% \\usepackage{hyperref}\n",
            "\\PassOptionsToPackage{dvipsnames}{xcolor}\n",
            "\\usepackage[dvipsnames]{xcolor}\n",
            "\\RequirePackage{amsmath}\n",
            "\\usepackage{xcolor}\n",
            "Body\n",
        );
        let transform = ensure_packages(source);
        let xcolors = transform
            .source
            .lines()
            .filter(|line| line.contains("xcolor") && line.trim_start().starts_with("\\usepackage"))
            .count();
        assert_eq!(xcolors, 1);
        assert!(transform.source.contains("\\usepackage{hyperref}"));
        assert_eq!(transform.line_map.iter().filter(|line| **line == 0).count(), 7);
    }

    #[test]
    fn tex_diagnostic_mapping_does_not_shift_lines_before_injection() {
        let log = "! Undefined control sequence.\nl.1 \\bad\n! Missing } inserted.\nl.4 body\n";
        let diagnostics = parse_tex_log(log, &[1, 2, 0, 3]);
        assert_eq!(diagnostics[0]["line"], 1);
        assert_eq!(diagnostics[1]["line"], 3);
    }

