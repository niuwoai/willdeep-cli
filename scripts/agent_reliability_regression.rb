#!/usr/bin/env ruby
# frozen_string_literal: true

# Offline orchestration of the existing reliability tests, not a model benchmark.
# ruby scripts/agent_reliability_regression.rb --list
# ruby scripts/agent_reliability_regression.rb --check-manifest
# ruby scripts/agent_reliability_regression.rb --with-browser --out target/core-regression
# Browser prerequisites and any additional arguments belong to the existing browser
# test entrypoint; pass them with repeated --browser-arg (no shell interpretation).
require_relative 'lib/model_eval/regression'

exit ModelEval::Regression.main(ARGV) if $PROGRAM_NAME == __FILE__
