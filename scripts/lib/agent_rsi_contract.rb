# frozen_string_literal: true
require 'json'
require 'digest'

module ContractGolden
  module_function

  def canonical(value)
    case value
    when Hash
      '{' + value.keys.sort.map { |key| canonical(key) + ':' + canonical(value.fetch(key)) }.join(',') + '}'
    when Array
      '[' + value.map { |item| canonical(item) }.join(',') + ']'
    when String
      '"' + value.encode(Encoding::UTF_16BE).unpack('n*').map { |unit|
        case unit
        when 34 then '\\"'
        when 92 then '\\\\'
        when 32..126 then unit.chr
        else format('\\u%04x', unit)
        end
      }.join + '"'
    when Integer
      raise 'integer outside safe range' unless (0..9_007_199_254_740_991).cover?(value)
      value.to_s
    when TrueClass then 'true'
    when FalseClass then 'false'
    when NilClass then 'null'
    else raise "unsupported canonical value: #{value.class}"
    end
  end

  def hash(value)
    Digest::SHA256.hexdigest(canonical(value))
  end

end
