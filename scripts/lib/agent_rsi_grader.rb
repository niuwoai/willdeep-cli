# frozen_string_literal: true
require 'json'
require 'digest'
require_relative 'agent_rsi_contract'

module AgentRsiLive
  # Pure local parsing: no Account, Host, Provider or network dependencies.
  module Grader
    REVISION = 'report-json-v5'
    ANSWER_KEYS = %w[has_defect line reason].freeze
    PLACEHOLDERS = ['...', '…'].freeze
    module_function

    class UniqueObject < Hash
      def []=(key, value)
        raise JSON::ParserError, 'duplicate object key' if key?(key)
        super
      end
    end

    def balanced_object(text, start)
      depth = 0
      quoted = escaped = false
      (start...text.length).each do |index|
        character = text[index]
        if quoted
          if escaped then escaped = false
          elsif character == '\\' then escaped = true
          elsif character == '"' then quoted = false
          end
        elsif character == '"' then quoted = true
        elsif character == '{' then depth += 1
        elsif character == '}'
          depth -= 1
          return index if depth.zero?
        end
      end
      nil
    end

    def parse(fragment)
      JSON.parse(fragment, object_class: UniqueObject, allow_duplicate_key: false)
    end

    def shape?(answer)
      answer.is_a?(Hash) && answer.keys.sort == ANSWER_KEYS &&
        [true, false].include?(answer['has_defect']) && answer['reason'].is_a?(String) && !answer['reason'].strip.empty? &&
        (answer['line'].nil? || (answer['line'].is_a?(Integer) && answer['line'].positive?)) &&
        (answer['has_defect'] ? !answer['line'].nil? : answer['line'].nil?)
    end

    def answer_fields?(value)
      case value
      when Hash then !(value.keys & ANSWER_KEYS).empty? || value.values.any? { |v| answer_fields?(v) }
      when Array then value.any? { |v| answer_fields?(v) }
      else false
      end
    end

    class ReportParser
      def initialize
        @answers, @ignored, @error = [], Hash.new(0), nil
      end

      def extract(text)
        return {'answers' => [], 'ignored' => {}, 'error' => 'invalid_report_type'} unless text.is_a?(String)
        scan(text, :prose)
        {'answers' => @error ? [] : @answers, 'ignored' => @ignored, 'error' => @error}
      end

      def scan(text, context)
        index = 0
        while index < text.length && !@error
          if context == :prose && (span = code_span(text, index))
            scan(span[:body], span[:context])
            index = span[:ending]
          elsif text[index] == '{'
            index = object(text, index, context)
          else
            index += 1
          end
        end
      end

      def code_span(text, index)
        return unless ['`', '~'].include?(text[index])
        run = text[index..].match(/\A(`+|~+)/)[0]
        line_prefix = text[(text[0...index].rindex("\n") || -1) + 1...index]
        if run.length >= 3 && line_prefix.match?(/\A {0,3}\z/) && (opening = text[index..].match(/\A[`~]+([^\n]*)\n/))
          start = index + opening[0].length
          closing = text.match(/^[ ]{0,3}#{Regexp.escape(run[0])}{#{run.length},}[ \t]*$/, start)
          return unclosed_span(text) unless closing
          language = opening[1].strip.downcase
          return {body: text[start...closing.begin(0)], context: %w[json application/json].include?(language) ? :json : :code,
                  ending: closing.end(0)}
        end
        return unless run[0] == '`'
        closing = inline_ending(text, index + run.length, run)
        return unclosed_span(text) unless closing
        {body: text[(index + run.length)...closing], context: :inline, ending: closing + run.length}
      end

      def inline_ending(text, index, run)
        while index < text.length
          if text[index] == '{' && (ending = Grader.balanced_object(text, index))
            begin
              Grader.parse(text[index..ending]) # JSON string backticks are literal.
              index = ending + 1
              next
            rescue JSON::ParserError
              # Non-JSON code cannot consume the closing Markdown delimiter.
            end
          end
          return index if text[index, run.length] == run && text[index - 1] != '`' && text[index + run.length] != '`'
          index += 1
        end
        nil
      end

      def unclosed_span(text)
        @error = 'unclosed_code_span'
        {body: '', context: :code, ending: text.length}
      end

      def code_context?(text, index, fragment, context)
        return false if context == :json
        return true if context == :code
        prefix = text[[index - 200, 0].max...index]
        (context == :inline && prefix.end_with?('$')) ||
          prefix.match?(/(?:\b(?:function|for|if|while|switch|catch)\b[^\n]*|\b(?:try|finally|else|do)|=>|\breturn)\s*\z/) ||
          fragment.match?(/\b(?:return|throw|const|let|var)\b|;|=>|\A\{\s*[a-zA-Z_$][\w$]*\s*:/) ||
          (context == :inline && fragment == '{}')
      end

      def inline_template?(fragment, context)
        return false unless context == :inline
        if fragment.match?(/,\s*(?:\.\.\.|…)\s*\}\z/)
          value = Grader.parse(fragment.sub(/,\s*(?:\.\.\.|…)\s*\}\z/, '}'))
          value.keys.sort == %w[has_defect line] && [true, false].include?(value['has_defect']) &&
            (value['has_defect'] ? value['line'].is_a?(Integer) && value['line'].positive? : value['line'].nil?)
        elsif fragment.match?(/"reason"\s*:\s*(?:\.\.\.|…)(?=\s*[,}])/)
          value = Grader.parse(fragment.sub(/("reason"\s*:\s*)(?:\.\.\.|…)(?=\s*[,}])/, '\\1"..."'))
          Grader.shape?(value) && value['reason'] == '...'
        else
          false
        end
      rescue JSON::ParserError
        false
      end

      def object(text, index, context)
        ending = Grader.balanced_object(text, index)
        fragment = ending ? text[index..ending] : text[index..]
        unless ending
          return ignore_code(index) if context != :prose && code_context?(text, index, fragment, context) && !answer_like?(fragment)
          @error = 'incomplete_json_or_unknown_brace'
          return text.length
        end
        value = Grader.parse(fragment)
        if context == :inline && Grader.shape?(value) && PLACEHOLDERS.include?(value['reason'].strip)
          @ignored['inline_answer_template'] += 1
        elsif !Grader.answer_fields?(value) && code_context?(text, index, fragment, context)
          @ignored['code_brace'] += 1
        else
          @answers << value
        end
        ending + 1
      rescue JSON::ParserError
        if inline_template?(fragment, context)
          @ignored['inline_answer_template'] += 1
          ending + 1
        elsif code_context?(text, index, fragment, context) && !answer_like?(fragment)
          ignore_code(index)
        else
          @error = 'invalid_json_or_duplicate_key'
          text.length
        end
      end

      def answer_like?(fragment)
        fragment.match?(/\A\{\s*["']/)
      end

      def ignore_code(index)
        @ignored['code_brace'] += 1
        index + 1 # Keep inspecting inner objects; never hide a second answer.
      end
    end

    def extract(text) = ReportParser.new.extract(text)
    def objects(text) = extract(text)['answers']

    def grade(raw, expected, read_observed:, fixture_unchanged:)
      extraction = extract(raw)
      answers = extraction['answers']
      answer = answers.length == 1 ? answers.first : nil
      valid = shape?(answer)
      classification = valid && answer['has_defect'] == expected['has_defect']
      localization = classification && answer['line'] == expected['line']
      {'schema' => 'willdeep.agent-rsi.host-verifier.v1', 'parser_revision' => REVISION,
       'schema_valid' => !!valid, 'classification_correct' => !!classification, 'localization_correct' => !!localization,
       'read_observed' => read_observed == true, 'fixture_unchanged' => fixture_unchanged == true,
       'passed' => !!(localization && read_observed && fixture_unchanged),
       'raw_model_sha256' => Digest::SHA256.hexdigest(raw), 'oracle_sha256' => ContractGolden.hash(expected),
       'format_diagnostic' => extraction.slice('ignored', 'error').merge('complete_objects' => answers.length),
       'claim_limit' => 'No semantic reason/reproduction grading; exact seeded line is auxiliary.'}
    end
  end
end
