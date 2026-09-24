#!/usr/bin/env python3
import os
import logging
import argparse
import webbrowser


_LOGGER = logging.getLogger()


def generate_cov_report(open_report: bool, format: str, package: str):
    logging.basicConfig(level=logging.INFO)
    os.environ["RUSTFLAGS"] = "-Cinstrument-coverage"
    os.environ["LLVM_PROFILE_FILE"] = "target/coverage/%p-%m.profraw"
    _LOGGER.info("Executing tests with coverage")
    os.system(f"cargo test -p {package}")

    out_path = "./target/debug/coverage"
    if format == "lcov":
        out_path = "./target/debug/lcov.info"
    grcov_cmd = (
        f"grcov . -s . --binary-path ./target/debug/ -t {format} --branch --ignore-not-existing "
        f"-o {out_path}"
    )
    print(f"Running: {grcov_cmd}")
    os.system(grcov_cmd)
    if format == "lcov":
        lcov_cmd = (
            "genhtml -o ./target/debug/coverage/ --show-details --highlight --ignore-errors source "
            "--legend ./target/debug/lcov.info"
        )
        print(f"Running: {lcov_cmd}")
        os.system(lcov_cmd)
    if open_report:
        coverage_report_path = os.path.abspath("./target/debug/coverage/index.html")
        webbrowser.open_new_tab(coverage_report_path)
    _LOGGER.info("Done")


def main():
    parser = argparse.ArgumentParser(
        description="Generate coverage report and optionally open it in a browser"
    )
    parser.add_argument(
        "--open", action="store_true", help="Open the coverage report in a browser"
    )
    parser.add_argument(
        "-p",
        "--package",
        choices=["satrs", "minisim", "example-std"],
        default="satrs",
        help="Choose project to generate coverage for",
    )
    parser.add_argument(
        "--format",
        choices=["html", "lcov"],
        default="html",
        help="Choose report format (html or lcov)",
    )
    args = parser.parse_args()
    generate_cov_report(args.open, args.format, args.package)


if __name__ == "__main__":
    main()
