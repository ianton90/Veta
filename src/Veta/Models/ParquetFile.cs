using System.Collections.Generic;
using System.IO;
using System.Threading.Tasks;
using Parquet;
using Parquet.Schema;
using ParquetColumn = Parquet.Data.DataColumn;

namespace Veta.Models;

public class ParquetFile
{
    public string FilePath { get; }
    public string[] ColumnNames { get; }
    public List<Dictionary<string, object?>> Rows { get; }

    private ParquetFile(string filePath, string[] columnNames, List<Dictionary<string, object?>> rows)
    {
        FilePath = filePath;
        ColumnNames = columnNames;
        Rows = rows;
    }

    public static async Task<ParquetFile> OpenAsync(string filePath)
    {
        using ParquetReader reader = await ParquetReader.CreateAsync(filePath);
        DataField[] fields = reader.Schema.GetDataFields();

        string[] columnNames = new string[fields.Length];
        for (int i = 0; i < fields.Length; i++)
            columnNames[i] = fields[i].Name;

        List<Dictionary<string, object?>> rows = [];

        for (int i = 0; i < reader.RowGroupCount; i++)
        {
            using ParquetRowGroupReader rowGroup = reader.OpenRowGroupReader(i);
            ParquetColumn[] columns = new ParquetColumn[fields.Length];
            for (int j = 0; j < fields.Length; j++)
                columns[j] = await rowGroup.ReadColumnAsync(fields[j]);

            int rowCount = columns[0].Data.Length;
            for (int r = 0; r < rowCount; r++)
            {
                Dictionary<string, object?> row = [];
                for (int c = 0; c < fields.Length; c++)
                    row[columnNames[c]] = columns[c].Data.GetValue(r);
                rows.Add(row);
            }
        }

        return new ParquetFile(filePath, columnNames, rows);
    }
}
